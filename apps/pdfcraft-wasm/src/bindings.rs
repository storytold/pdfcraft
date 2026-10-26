//! The wasm-bindgen layer: JS types in, [`crate::core`] calls, JS types out. Nothing else lives
//! here, so there is nothing to test that the core's tests do not cover.

use wasm_bindgen::prelude::*;

use crate::core::{Document, Raster};
use crate::editing::{Kind, Target, TextStyle, parse_targets};

/// Seconds since the Unix epoch, from the page's clock (wasm has no system clock).
fn now() -> i64 {
    (js_sys::Date::now() / 1000.0) as i64
}

fn js(message: String) -> JsError {
    JsError::new(&message)
}

/// A box passed as `[x0, y0, x1, y1]`.
fn rect(v: &[f64]) -> Result<[f64; 4], JsError> {
    match v {
        [a, b, c, d] => Ok([*a, *b, *c, *d]),
        _ => Err(js("a box is [x0, y0, x1, y1]".into())),
    }
}

/// A rendered page or tile.
#[wasm_bindgen]
pub struct Rendered {
    inner: Raster,
}

#[wasm_bindgen]
impl Rendered {
    #[wasm_bindgen(getter)]
    pub fn width(&self) -> u32 {
        self.inner.width
    }

    #[wasm_bindgen(getter)]
    pub fn height(&self) -> u32 {
        self.inner.height
    }

    /// Why the picture is partial or approximate, if it is: `content-truncated`, `colour-fallback`.
    #[wasm_bindgen(getter)]
    pub fn warnings(&self) -> Vec<String> {
        self.inner.warnings.iter().map(|w| (*w).to_string()).collect()
    }

    /// Premultiplied RGBA8 pixels, row by row.
    pub fn take_rgba(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.inner.rgba)
    }
}

/// An open PDF.
#[wasm_bindgen]
pub struct Pdf {
    inner: Document,
}

#[wasm_bindgen]
impl Pdf {
    /// Opens a PDF. The error message starts with `needs-password:`, `wrong-password:` or
    /// `invalid:`, so the page can ask for a password and try again.
    #[wasm_bindgen(constructor)]
    pub fn new(bytes: &[u8], password: Option<String>) -> Result<Pdf, JsError> {
        match Document::open(bytes, password.as_deref(), now) {
            Ok(inner) => Ok(Pdf { inner }),
            Err(e) => Err(JsError::new(&format!("{}: {}", e.code(), e.message()))),
        }
    }

    pub fn page_count(&self) -> usize {
        self.inner.page_count()
    }

    /// `[width, height]` in points, or empty for a page that does not exist.
    pub fn page_size(&self, page: usize) -> Vec<f32> {
        self.inner.page_size(page).map(|(w, h)| vec![w, h]).unwrap_or_default()
    }

    pub fn is_encrypted(&self) -> bool {
        self.inner.is_encrypted()
    }

    /// Draws a page at `scale` device pixels per point. Pass `tile` as `[x, y, width, height]`
    /// in device pixels to draw only that region.
    pub fn render(&mut self, page: usize, scale: f32, tile: Option<Vec<u32>>) -> Result<Rendered, JsError> {
        let tile = match tile.as_deref() {
            Some([x, y, w, h]) => Some((*x, *y, *w, *h)),
            Some(_) => return Err(js("a tile is [x, y, width, height]".into())),
            None => None,
        };
        self.inner.render(page, scale, tile).map(|inner| Rendered { inner }).map_err(js)
    }

    /// The page's text blocks as JSON (see `Document::text_blocks_json`).
    pub fn text_blocks(&self, page: usize) -> Result<String, JsError> {
        self.inner.text_blocks_json(page).map_err(js)
    }

    pub fn edit_text_block(&mut self, page: usize, block: usize, text: &str) -> Result<(), JsError> {
        self.inner.edit_text_block(page, block, text).map_err(js)
    }

    pub fn delete_pages(&mut self, pages: Vec<usize>) -> Result<(), JsError> {
        self.inner.delete_pages(&pages).map_err(js)
    }

    pub fn rotate_pages(&mut self, pages: Vec<usize>, degrees: i32) -> Result<(), JsError> {
        self.inner.rotate_pages(&pages, i64::from(degrees)).map_err(js)
    }

    pub fn move_pages(&mut self, pages: Vec<usize>, to: usize) -> Result<(), JsError> {
        self.inner.move_pages(&pages, to).map_err(js)
    }

    pub fn insert_blank_page(&mut self, at: usize, width: f64, height: f64) -> Result<(), JsError> {
        self.inner.insert_blank_page(at, width, height).map_err(js)
    }

    /// The page's editable objects as JSON: `kind`, `index`, `type`, `text` and `view` (the box where
    /// it is drawn, in points from the page's top-left).
    pub fn objects(&self, page: usize) -> Result<String, JsError> {
        self.inner.objects_json(page).map_err(js)
    }

    /// Moves objects by (dx, dy) displayed points. `targets` is `[{"kind": "text", "index": 0}, ...]`.
    pub fn move_objects(&mut self, page: usize, targets: &str, dx: f64, dy: f64) -> Result<(), JsError> {
        let targets = parse_targets(targets).map_err(js)?;
        self.inner.move_objects(page, &targets, dx, dy).map_err(js)
    }

    /// Gives an object the displayed box `[x0, y0, x1, y1]`.
    pub fn resize_object(&mut self, page: usize, kind: &str, index: usize, view: Vec<f64>) -> Result<(), JsError> {
        let target = Target { kind: Kind::parse(kind).map_err(js)?, index };
        self.inner.resize_object(page, target, rect(&view)?).map_err(js)
    }

    /// Adds a text box over the displayed box. `style` is JSON: family, bold, italic, size, color, align.
    pub fn add_text(&mut self, page: usize, view: Vec<f64>, text: &str, style: &str) -> Result<(), JsError> {
        let style = TextStyle::parse(style).map_err(js)?;
        self.inner.add_text(page, rect(&view)?, text, &style).map_err(js)
    }

    /// Adds a picture over `[x0, y0, x1, y1]`, or centred at its natural size when no box is given.
    pub fn add_image(&mut self, page: usize, name: &str, bytes: &[u8], view: Option<Vec<f64>>) -> Result<(), JsError> {
        let view = view.as_deref().map(rect).transpose()?;
        self.inner.add_image(page, name, bytes, view).map_err(js)
    }

    pub fn delete_object(&mut self, page: usize, kind: &str, index: usize) -> Result<(), JsError> {
        let target = Target { kind: Kind::parse(kind).map_err(js)?, index };
        self.inner.delete_object(page, target).map_err(js)
    }

    /// Rewrites an object's text and/or restyles it; `text` left out keeps the words.
    pub fn set_text(&mut self, page: usize, kind: &str, index: usize, text: Option<String>, style: &str) -> Result<(), JsError> {
        let target = Target { kind: Kind::parse(kind).map_err(js)?, index };
        let style = TextStyle::parse(style).map_err(js)?;
        self.inner.set_text(page, target, text.as_deref(), &style).map_err(js)
    }

    /// Paints a white box over the displayed box.
    pub fn cover(&mut self, page: usize, view: Vec<f64>) -> Result<(), JsError> {
        self.inner.cover(page, rect(&view)?).map_err(js)
    }

    /// The label of the edit `undo` would take back, or nothing.
    pub fn can_undo(&self) -> Option<String> {
        self.inner.can_undo()
    }

    pub fn can_redo(&self) -> Option<String> {
        self.inner.can_redo()
    }

    pub fn undo(&mut self) -> Result<(), JsError> {
        self.inner.undo().map_err(js)
    }

    pub fn redo(&mut self) -> Result<(), JsError> {
        self.inner.redo().map_err(js)
    }

    /// The file as an incremental update: the original bytes stay as a prefix.
    pub fn save(&self) -> Result<Vec<u8>, JsError> {
        self.inner.save().map_err(js)
    }

    /// The file rewritten in full.
    pub fn save_full(&self) -> Result<Vec<u8>, JsError> {
        self.inner.save_full().map_err(js)
    }
}
