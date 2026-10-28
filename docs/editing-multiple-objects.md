# Move several objects together

In **Edit a PDF → Edit text & images**, select several items on the same page:

- **Shift-click**, **Ctrl-click**, or **Command-click** toggles an item. An ordinary selected
  paragraph, image, added item, or inline vector graphic becomes the first member when you extend
  its selection.
- Drag through empty page space to draw a selection rectangle. Items intersecting the rectangle
  are selected; hold one of those modifiers to add to the selection.
- An inline vector graphic (an icon or logo painted directly by the page's paths rather than an
  image or Form) has no single-object tool of its own: an ordinary click selects it and the same
  press starts a move, while Delete or Backspace removes only its paths. A single selected graphic
  shows eight resize handles: the four corners keep its aspect ratio, and the midpoint of each
  side stretches only that axis. Either way the box may grow past the page edge.
- Drag the selection to move all its items by the same distance. Blue outlines preview the move;
  releasing commits it as **one undo step**. Escape cancels the gesture or clears the selection.
- Switching tabs, opening Home or a modal dialog, or losing window focus cancels a drag preview.
  Returning to the document keeps the selection without committing a release from elsewhere.
- Modifier-clicks prefer added items and text over background artwork, so labels remain selectable.
- Clear the selection or double-click an item to return to ordinary text editing and image resizing.
  Finish or discard a text draft before starting a multi-selection.

The supported items are the existing paragraph boxes, raster Image XObjects, whole Form artwork,
inline vector graphics, and text/images added by PdfCraft. A Form remains one item, including its
nested artwork and resources. A graphic is either a whole `q … Q` scope that paints only paths
(its separate fills stay one icon), or one path painted in a scope shared with text or pictures.
Clip-only paths and artwork covering most of the page are background, not selectable items.
Added items appear once in the selection inventory. Selections stay on one page and contain at
most 1,000 items; pages with more than 10,000 recognised objects are refused for group selection.
Content streams must decode completely within 64 MiB each and 128 MiB per page, with a 1,024-level
graphics-state limit and 100,000 content operations. These limits apply to both the original page
and the moved or resized result; unreadable or missing streams are refused. Moving between pages,
inline images, group resizing, align/distribute, and vertical or clipping text are outside this
operation's scope. A refusal changes nothing.

Added streams move by changing their outer placement, preserving drawing bytes, fonts, crop paths,
and image transforms even if the page was rotated after the item was added. Their original local
coordinate basis is retained in `/PCAdded/MoveOrigin` so metadata and source placement agree after
repeated moves. Ordinary content updates replace that stream and reset the basis.

Existing text moves without retyping, font substitution, or reflow: its original encoded glyph
strings and kerning are replayed with temporary numeric `TJ` adjustments and text rise. The reader
performs the original glyph advances, so following unselected text keeps its position. Paragraph
indices can change when detection groups lines differently after a move. The UI remaps its own
selection only when the new geometry matches uniquely; other document edits clear stale selection.

## Engine

`Document::editable_objects(page)` returns a mixed inventory with zero-based `ObjectTarget`
references and user-space rectangles. Apply `Edit::MoveObjects { page, objects, offset }` through
`Session`. Targets resolve together before mutation. Permission checks, dirty state, history,
incremental saving, and refreshing use the same transaction as other edits.

`Document::page_graphics(page)` returns the inline vector graphics with their visible rectangles,
in drawing order. Apply `Edit::DeletePageGraphic { page, index }` to remove one. A whole pure
`q … Q` scope goes with its saved graphics state; a path painted in a shared scope loses only its
path and paint operators, so the colour, clip, text state and marked content around it stay. The
edit is one undo step and is refused without a partial change when the index no longer exists.

`Edit::ResizePageGraphic { page, index, rect }` gives one graphic the user-space `rect` it should
occupy. Its paths scale about the old box's top-left corner, so a point that was there stays
there; stroke widths, dashes and colours keep their own size rather than scaling with the box.
A whole `q … Q` graphic moves and scales as one piece, so a clip box that wraps its artwork (as
pasted or exported icons often have) travels with it: enlarging is not cut off at the old box,
and moving does not leave the clip behind.
A degenerate or unchanged box and a stale index are refused without changing the document, and
the resize is one undo step. The UI keeps a graphic's aspect ratio while dragging a corner
handle and changes one axis freely from a side handle; the engine API accepts any box with
positive width and height.

`Edit::RestackObject { page, object, direction }` changes where a single object sits in the
page's paint order: `Restack::Front`, `Forward`, `Backward` or `Back`. `Edit::RestackObjects { page,
objects, direction }` does the same for several objects as one group: it keeps their relative paint
order and moves the group above the level after its highest member, below the level before its
lowest member, or to the front or back as a unit. A PDF paints in stream order, so only the
objects' own drawing operators move; everything else is copied byte for byte, in the same order.
A paragraph, picture or graphic painted inside a `q … Q` scope (or the `BT … ET` text object of a
paragraph) that holds nothing else moves as a whole, so its clip box and nested state travel with
it. Content in a scope shared with other objects moves just its own operators, wrapped in `q`, the
paint state it inherited and a `cm` that restores its page-space transform at the new level, then
`Q`, so its colour, text state, transparency and position stay. An added item moves as a whole
`/Contents` entry with its `/PCAdded` parameters, so it remains editable. `Front` inserts after all
content, above marks already there; `Back` inserts after a leading background or watermark but
below everything else. The move is one undo step and is a no-op when the object or group is already
at that end. It is refused as a whole when an object shares its text object or content stream with
content that would have to move with it, and when any target no longer exists. The Edit panel always
shows a **Manage levels** section with the four actions, with the Lucide `arrow-up-to-line`,
`chevron-up`, `chevron-down` and `arrow-down-to-line` icons. With nothing selected they are drawn
disabled in the faint text colour; a paragraph opened for editing, a picture, an added item, an
inline graphic, an object or a group enables them. Pressing one keeps the object or group selected,
so repeated presses move it several levels.

`Document::object_move_offset(page, [dx, dy])` converts a displayed-page vector (points, right/down)
to the user-space vector required by the edit, accounting for the crop box, `/Rotate`, and `/UserUnit`.
A frontend with an additional view rotation converts its pointer positions to displayed page space
first. Duplicate, missing, non-finite, oversized, or unsupported targets fail atomically. Engine callers
re-list after changes; the automation generation guard detects stale references before the edit.

## CLI and MCP

Both frontends use the same tools. `object_list` returns a document `generation` and items with
`kind` (`added`, `text`, `image`, or `graphic`), 1-based `index`, `type`, displayed `rect`, and
optional `text`. Treat those references as belonging to that generation and pass it back when
moving, resizing or deleting:

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

Use the actual generation and references from your document, rather than assuming these example
values. `offset` is in displayed points: positive x moves right, positive y moves down. The optional
generation guard rejects a changed document before interpreting references. Re-list after any edit,
undo, redo, or save; there is no claim that an index persists across generations. A `graphic`
reference moves with the rest of the group. A graphic is resized or deleted with its own tools,
using the same generation guard:

```json
[
  {"tool":"graphic_resize","args":{
    "doc":1,"page":1,"generation":0,
    "index":3,"rect":[120,80,180,120]
  }},
  {"tool":"doc_save","args":{"doc":1,"path":"resized.pdf"}}
]
```

`graphic_resize` takes the displayed box it should occupy, `[left, top, right, bottom]` in points
(positive width and height). `graphic_delete`, with the same `index` and `generation`, removes the
paths instead. `graphic_resize` scales them about the old box's top-left corner while stroke widths
and colours keep their size. `object_restack` moves one item by `kind`/`index` in the paint order,
using `direction` `front`, `forward`, `backward` or `back`; it changes no geometry. Give `objects`
(a list of `{kind, index}`) instead of `kind`/`index` to move a group together, keeping its order:

```json
[
  {"tool":"object_restack","args":{
    "doc":1,"page":1,"generation":0,
    "objects":[{"kind":"text","index":1},{"kind":"image","index":1}],
    "direction":"forward"
  }},
  {"tool":"doc_save","args":{"doc":1,"path":"restacked.pdf"}}
]
```

A single reference keeps the same shape as before:

```json
[
  {"tool":"object_restack","args":{
    "doc":1,"page":1,"generation":0,
    "kind":"text","index":1,"direction":"front"
  }},
  {"tool":"doc_save","args":{"doc":1,"path":"restacked.pdf"}}
]
```

A stale `generation`, an unknown reference or a refusal changes nothing. Run a script with
`pdfcraft-cli run --script steps.json --root DIR` or call these tools through the opt-in MCP
server.

Regression coverage: `crates/edit/src/tests.rs` (glyph bytes, neighbours, split streams, shared Forms,
rotated/cropped/UserUnit pages, graphic grouping, movement, resizing, restacking and refusals),
`crates/engine/src/tests.rs` (permissions, history, rendering and save/reopen),
`crates/automation/tests/it/automation.rs` (generation guards and hostile references), and
`crates/ui-egui/tests/it/object_selection.rs` (real-shell selection, graphics and gestures).
