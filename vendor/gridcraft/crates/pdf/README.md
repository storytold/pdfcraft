# gridcraft-pdf

A small PDF 1.7 writer for shaped Arabic text, vector drawing and RGBA images.

## API migration

The geometry-taking methods now use `Point` and `Rect` values to name their coordinates, and
`image_rgba` groups its pixel dimensions into an array. Update calls to the four changed methods as
follows:

```rust
use gridcraft_pdf::{Font, PdfDoc, Point, Rect};

let mut doc = PdfDoc::new();
let page = doc.add_page(612.0, 792.0);

// Before: page.link_page(20.0, 30.0, 80.0, 16.0, 1, 40.0, 50.0);
page.link_page(Rect::new(20.0, 30.0, 80.0, 16.0), 1, Point::new(40.0, 50.0));

// Before: page.stroke_line_dashed(0.0, 0.0, 100.0, 40.0, 1.0, [0, 0, 0], &[4.0, 2.0]);
page.stroke_line_dashed(Point::new(0.0, 0.0), Point::new(100.0, 40.0), 1.0, [0, 0, 0], &[4.0, 2.0]);

// Before: page.text_rotated(20.0, 40.0, 12.0, Font::Helvetica, [0, 0, 0], "Label", 0.2);
page.text_rotated(Point::new(20.0, 40.0), 12.0, Font::Helvetica, [0, 0, 0], "Label", 0.2);

let rgba = [255, 0, 0, 255];
// Before: page.image_rgba(10.0, 10.0, 24.0, 24.0, &rgba, 1, 1);
page.image_rgba(Rect::new(10.0, 10.0, 24.0, 24.0), &rgba, [1, 1]);
```

`Point::new(x, y)` uses the page's top-left origin, where y increases downward. `Rect::new(x, y,
width, height)` describes the destination or link rectangle in the same coordinate system. The
`image_rgba` pixel-size array is `[width, height]`.
