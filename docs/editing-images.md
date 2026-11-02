# Editing existing images and figures

In **Edit a PDF → Edit text and images**, click a figure to select it. Drag the selected
figure to move it, drag a corner to resize it proportionally, or press Delete to remove
that placement. The context menu also rotates or flips it. Changes support undo and
survive saving/reopening. For single-figure transforms, a clipping path exclusive to one image moves and resizes
together with its pixels, including nested graphics states, intervening state operators
and paths split across content streams. A clip shared with other content stays where it is.
When the figure already sits alone in a `q … cm … Q` (its own placement, or the one an
earlier edit added), moving or resizing it again changes that `cm`, so repeated edits don't
nest one more graphics state each time. Moving a privately cropped figure again likewise
changes the `cm` before its clip instead of adding another.

PDF figures are not always bitmap images. Imported diagrams often use a **Form XObject**,
which packages text, vector paths, and nested images into one object. PdfCraft selects
this object using its `BBox` and optional `Matrix` and edits it as a whole. Its original
content, resources, clipping box, and transparency settings remain intact. Moving or
deleting one use of a shared figure does not alter its other placements or pages.

**Replace Image** and **Save Image As** apply only to raster Image XObjects; these menu
items are disabled for grouped Form artwork. The tool does not ungroup Forms, edit their
individual internal elements, select arbitrary ungrouped vector paths, or edit inline
images (`BI`/`ID`/`EI`). A Form's bounding box may include whitespace.

For automation, `page_images` lists both kinds with `kind: "image"` or `kind: "form"`.
Forms have `pixels: [0, 0]`, since they have no single bitmap resolution. Use the returned
number with `image_edit` (move, rotate, flip_horizontal, flip_vertical, delete).
Coordinates are top-left-origin page points. `image_save` and `image_edit`'s replace action
return a clear error for Form artwork.

Selection frames follow the visible portion of artwork after page and path clipping. Clicking
editable text takes precedence over overlapping artwork, including labels inside an image's
visible rectangle. Hidden image placements have no selection frame. Bezier extrema determine
curved-path frames. Hit testing evaluates the actual clipping paths with their nonzero or
even-odd fill rule, so holes and empty corners do not intercept clicks. Intersections of several
curved clips can still have a loose bounding frame. Dragging the visible frame moves or resizes
the full placement by the same change; resizing keeps the opposite corner fixed, including
horizontal-only or vertical-only corner drags, and grabbing a grip slightly off-centre does not
jump. A selected corner keeps control of its drag even over a text label; the drag previews one
frame and applies one undoable edit on release. Exclusive crops and their holes move
with the selected image, also when the picture sits inside a clip it shares with its section
(a report drawing each picture as `q re W n cm Do Q` within a table's clip): the picture's own
clip scales with it, and shared clips stay fixed without moving neighbouring text or artwork.

`image_hit` applies the same path and hole checks headlessly. `page_images` retains `rect` as
the full placement rectangle used by `image_edit` and adds `visible_rect` for selection (null
when fully clipped). `object_list` reports visible artwork bounds while keeping the original
source indexes. A selection frame does not create, remove or enlarge the PDF's clipping path.
