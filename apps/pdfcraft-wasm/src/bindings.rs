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

    /// Adds the pages of another PDF so the first lands at page `at` (the page count appends them).
    pub fn insert_pages_from(&mut self, name: &str, bytes: &[u8], at: usize) -> Result<(), JsError> {
        self.inner.insert_pages_from(name, bytes, at).map_err(js)
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

    /// Turns a picture by quarter turns clockwise (negative for anticlockwise).
    pub fn rotate_object(&mut self, page: usize, kind: &str, index: usize, quarter_turns: i32) -> Result<(), JsError> {
        let target = Target { kind: Kind::parse(kind).map_err(js)?, index };
        self.inner.rotate_object(page, target, quarter_turns).map_err(js)
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

    /// Every match of `query` as JSON: `page`, `text` and `rects` (one per line it spans).
    pub fn find(&mut self, query: &str, case_sensitive: bool, whole_words: bool, limit: usize) -> Result<String, JsError> {
        self.inner.find_json(query, case_sensitive, whole_words, limit).map_err(js)
    }

    /// The text between two points `[x, y]` of a page, as `{ "text": ..., "rects": [...] }`.
    pub fn select_text(&mut self, page: usize, from: Vec<f64>, to: Vec<f64>) -> Result<String, JsError> {
        match (from.as_slice(), to.as_slice()) {
            ([a, b], [c, d]) => self.inner.select_text_json(page, [*a, *b], [*c, *d]).map_err(js),
            _ => Err(js("a point is [x, y]".into())),
        }
    }

    /// Marks text up (`highlight`, `underline` or `strikeout`). `rects` is JSON `[[x0, y0, x1, y1], ...]`
    /// (what `select_text` gave) and `color` is `[r, g, b]` from 0 to 1, or left out.
    pub fn markup(&mut self, page: usize, kind: &str, rects: &str, color: Option<Vec<f64>>) -> Result<(), JsError> {
        let rects: Vec<[f64; 4]> = serde_json::from_str(rects).map_err(|e| js(e.to_string()))?;
        let color = match color.as_deref() {
            Some([r, g, b]) => Some([*r, *g, *b]),
            Some(_) => return Err(js("a color is [r, g, b]".into())),
            None => None,
        };
        self.inner.markup(page, kind, &rects, color).map_err(js)
    }

    /// Adds a link over a box: an http(s) or mailto: address, or `page:N` (1-based).
    pub fn add_link(&mut self, page: usize, view: Vec<f64>, target: &str) -> Result<(), JsError> {
        self.inner.add_link(page, rect(&view)?, target).map_err(js)
    }

    /// The page's links as JSON: `index`, `view`, and `uri` or `page` (0-based).
    pub fn links(&self, page: usize) -> Result<String, JsError> {
        self.inner.links_json(page).map_err(js)
    }

    pub fn delete_link(&mut self, page: usize, index: usize) -> Result<(), JsError> {
        self.inner.delete_link(page, index).map_err(js)
    }

    /// The page's marks and comments as JSON: `index`, `subtype`, `view`, `contents`.
    pub fn annotations(&self, page: usize) -> Result<String, JsError> {
        self.inner.annotations_json(page).map_err(js)
    }

    /// Adds a comment from a JSON description (see the Rust docs of `add_annotation`).
    pub fn add_annotation(&mut self, page: usize, spec: &str) -> Result<(), JsError> {
        self.inner.add_annotation(page, spec).map_err(js)
    }

    /// The stamps that can be added, as JSON.
    pub fn stamps() -> String {
        Document::stamps_json()
    }

    /// Adds a form field over the displayed box `[x0, y0, x1, y1]` from a JSON description.
    pub fn add_field(&mut self, page: usize, view: Vec<f64>, spec: &str) -> Result<(), JsError> {
        self.inner.add_field(page, rect(&view)?, spec).map_err(js)
    }

    pub fn delete_field(&mut self, name: &str) -> Result<(), JsError> {
        self.inner.delete_field(name).map_err(js)
    }

    pub fn set_field_props(&mut self, name: &str, spec: &str) -> Result<(), JsError> {
        self.inner.set_field_props(name, spec).map_err(js)
    }

    /// Protects with passwords and permissions, from a JSON description; the next save is encrypted.
    pub fn protect(&mut self, spec: &str) -> Result<(), JsError> {
        self.inner.protect(spec).map_err(js)
    }

    pub fn remove_protection(&mut self) -> Result<(), JsError> {
        self.inner.remove_protection().map_err(js)
    }

    /// The layers as JSON: `index`, `name`, `visible`.
    pub fn layers(&self) -> Result<String, JsError> {
        self.inner.layers_json().map_err(js)
    }

    /// Shows or hides a layer for viewing; true when that changed what is shown.
    pub fn set_layer_visible(&mut self, index: usize, visible: bool) -> bool {
        self.inner.set_layer_visible(index, visible)
    }

    /// Puts a picture on the page as a stamp over the displayed box.
    pub fn add_picture_stamp(&mut self, page: usize, view: Vec<f64>, name: &str, bytes: &[u8]) -> Result<(), JsError> {
        self.inner.add_picture_stamp(page, rect(&view)?, name, bytes).map_err(js)
    }

    /// Bakes typed text, marks and signatures into the page.
    pub fn flatten_fill_sign(&mut self) -> Result<(), JsError> {
        self.inner.flatten_fill_sign().map_err(js)
    }

    /// The document's signatures as JSON.
    pub fn signatures(&self) -> Result<String, JsError> {
        self.inner.signatures_json().map_err(js)
    }

    /// Makes a new digital ID (self-signed, P-256) as a PKCS #12 file; `now` is the Unix time in seconds.
    pub fn create_digital_id(name: &str, organisation: &str, email: &str, password: &str, years: u32, now: f64) -> Result<Vec<u8>, JsError> {
        Document::create_digital_id(name, organisation, email, password, years, now as i64).map_err(js)
    }

    /// Signs with a PKCS #12 certificate file and its password; returns the signed PDF.
    pub fn sign_with_certificate(&mut self, p12: &[u8], password: &str, spec: &str) -> Result<Vec<u8>, JsError> {
        self.inner.sign_with_certificate(p12, password, spec).map_err(js)
    }

    pub fn set_annotation_contents(&mut self, page: usize, index: usize, text: &str) -> Result<(), JsError> {
        self.inner.set_annotation_contents(page, index, text).map_err(js)
    }

    pub fn delete_annotation(&mut self, page: usize, index: usize) -> Result<(), JsError> {
        self.inner.delete_annotation(page, index).map_err(js)
    }

    /// The page's form fields as JSON: `name`, `kind`, `value`, `view`.
    pub fn fields(&self, page: usize) -> Result<String, JsError> {
        self.inner.fields_json(page).map_err(js)
    }

    /// Fills a field: `value` is JSON, `{"text": ...}`, `{"check": true}` or `{"choice": [...]}`.
    pub fn set_field(&mut self, name: &str, value: &str) -> Result<(), JsError> {
        self.inner.set_field(name, value).map_err(js)
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
