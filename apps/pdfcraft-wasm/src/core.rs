//! One open document: render pages, read and rewrite text blocks, change pages, undo, save.
//! Page and block indices are 0-based. Every error is a message for the person, never a panic.

use std::sync::Arc;

use pdfcraft_engine::{BlockStyle, DocId, Edit, Session};
use pdfcraft_render::{OpenError, PageRenderer, RenderRequest, RequestKind, Tile};

/// Why a document did not open.
#[derive(Debug, PartialEq, Eq)]
pub enum OpenFailure {
    NeedsPassword,
    WrongPassword,
    Invalid(String),
}

impl OpenFailure {
    /// A stable code for the host page: `needs-password`, `wrong-password` or `invalid`.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NeedsPassword => "needs-password",
            Self::WrongPassword => "wrong-password",
            Self::Invalid(_) => "invalid",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::NeedsPassword => "the document is protected by a password".into(),
            Self::WrongPassword => "the password is incorrect".into(),
            Self::Invalid(m) => m.clone(),
        }
    }
}

/// A rendered page or tile: premultiplied RGBA8, row-major.
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub struct Document {
    session: Session,
    id: DocId,
    renderer: Option<(Arc<Vec<u8>>, PageRenderer)>,
}

impl Document {
    /// Opens `bytes`. `clock` gives the Unix time in seconds, which the engine stamps into saved
    /// files; a browser supplies `Date.now()` because wasm has no system clock.
    pub fn open(bytes: &[u8], password: Option<&str>, clock: fn() -> i64) -> Result<Self, OpenFailure> {
        let mut session = Session::new().with_clock(clock);
        let id = session.open("document.pdf", None, Arc::new(bytes.to_vec()), password).map_err(|e| match e {
            OpenError::NeedsPassword => OpenFailure::NeedsPassword,
            OpenError::WrongPassword => OpenFailure::WrongPassword,
            other => OpenFailure::Invalid(other.to_string()),
        })?;
        Ok(Self { session, id, renderer: None })
    }

    fn doc(&self) -> Result<&pdfcraft_engine::Document, String> {
        self.session.get(self.id).ok_or_else(|| "the document is closed".to_string())
    }

    pub fn page_count(&self) -> usize {
        self.doc().map(|d| d.info.pages.len()).unwrap_or(0)
    }

    /// The displayed size of a page in points, after its rotation.
    pub fn page_size(&self, page: usize) -> Option<(f32, f32)> {
        self.doc().ok()?.info.pages.get(page).map(|p| (p.width, p.height))
    }

    pub fn is_encrypted(&self) -> bool {
        self.doc().map(|d| d.info.encrypted).unwrap_or(false)
    }

    /// Draws page `page` at `scale` device pixels per point, or only `tile` (x, y, w, h in device
    /// pixels at that scale).
    pub fn render(&mut self, page: usize, scale: f32, tile: Option<(u32, u32, u32, u32)>) -> Result<Raster, String> {
        if !(scale.is_finite() && scale > 0.0) {
            return Err("the scale must be a positive number".into());
        }
        if page >= self.page_count() {
            return Err(format!("page {} does not exist", page.saturating_add(1)));
        }
        let display = self.doc()?.display.clone();
        let stale = self.renderer.as_ref().is_none_or(|(shown, _)| !Arc::ptr_eq(shown, &display));
        if stale {
            let config = self.session.render_config(self.id).unwrap_or_default();
            self.renderer = Some((display.clone(), PageRenderer::new(display, config)));
        }
        let (_, renderer) = self.renderer.as_mut().ok_or_else(|| "no renderer".to_string())?;
        let tile = tile.map(|(x, y, w, h)| Tile { x, y, w, h });
        let out = renderer.render(RenderRequest { page, kind: RequestKind::Pixels, tile, scale, tag: 0, preview: false });
        match out.error {
            Some(e) => Err(e),
            None => Ok(Raster { width: out.width, height: out.height, rgba: out.rgba.to_vec() }),
        }
    }

    /// The text blocks of a page as a JSON array, in reading order. Each has its `index`, the page
    /// `rect` as [x0, y0, x1, y1] in user space, the `text` the person edits, the font and style,
    /// and whether it is an invisible OCR layer.
    pub fn text_blocks_json(&self, page: usize) -> Result<String, String> {
        if page >= self.page_count() {
            return Err(format!("page {} does not exist", page.saturating_add(1)));
        }
        let blocks: Vec<serde_json::Value> = self
            .doc()?
            .text_blocks(page)
            .iter()
            .enumerate()
            .map(|(index, b)| {
                serde_json::json!({
                    "index": index,
                    "rect": b.rect,
                    "text": b.editor_text,
                    "font": b.base_font,
                    "size": b.size,
                    "bold": b.bold,
                    "italic": b.italic,
                    "color": b.color,
                    "rotation": b.rotation,
                    "invisible": b.invisible,
                })
            })
            .collect();
        serde_json::to_string(&blocks).map_err(|e| e.to_string())
    }

    fn apply(&mut self, edit: Edit) -> Result<(), String> {
        self.session.apply(self.id, edit).map_err(|e| e.to_string())
    }

    pub fn edit_text_block(&mut self, page: usize, block: usize, text: &str) -> Result<(), String> {
        self.apply(Edit::EditTextBlock { page, block, text: text.to_string(), style: BlockStyle::default() })
    }

    pub fn delete_pages(&mut self, pages: &[usize]) -> Result<(), String> {
        self.apply(Edit::DeletePages { pages: pages.to_vec() })
    }

    /// Turns `pages` clockwise by `degrees` (a multiple of 90).
    pub fn rotate_pages(&mut self, pages: &[usize], degrees: i64) -> Result<(), String> {
        self.apply(Edit::RotatePages { pages: pages.to_vec(), degrees })
    }

    /// Moves `pages` so the first of them lands at index `to` of the new order.
    pub fn move_pages(&mut self, pages: &[usize], to: usize) -> Result<(), String> {
        self.apply(Edit::MovePages { pages: pages.to_vec(), to })
    }

    pub fn insert_blank_page(&mut self, at: usize, width: f64, height: f64) -> Result<(), String> {
        self.apply(Edit::InsertBlankPage { at, width, height })
    }

    /// The label of the edit [`undo`](Self::undo) would take back, when there is one.
    pub fn can_undo(&self) -> Option<String> {
        self.doc().ok()?.can_undo().map(str::to_string)
    }

    pub fn can_redo(&self) -> Option<String> {
        self.doc().ok()?.can_redo().map(str::to_string)
    }

    pub fn undo(&mut self) -> Result<(), String> {
        self.session.undo(self.id).map(|_| ()).map_err(|e| e.to_string())
    }

    pub fn redo(&mut self) -> Result<(), String> {
        self.session.redo(self.id).map(|_| ()).map_err(|e| e.to_string())
    }

    /// The file as an incremental update: the original bytes stay as a prefix, so signatures on the
    /// original remain valid.
    pub fn save(&self) -> Result<Vec<u8>, String> {
        self.session.save_bytes(self.id).map(|b| b.to_vec()).map_err(|e| e.to_string())
    }

    /// The file rewritten in full, without the superseded objects.
    pub fn save_full(&self) -> Result<Vec<u8>, String> {
        self.session.save_full_bytes(self.id).map(|b| b.to_vec()).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal `n`-page document; page `i` shows "Page i+1".
    fn fixture(n: usize) -> Vec<u8> {
        let mut objs: Vec<Vec<u8>> = vec![b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()];
        let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
        objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 200 300] >>", kids.join(" ")).into_bytes());
        objs.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
        for i in 0..n {
            objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * i).into_bytes());
            let body = format!("BT /F1 24 Tf 20 150 Td (Page {}) Tj ET", i + 1);
            objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()).into_bytes());
        }
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(o);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    fn open(n: usize) -> Document {
        Document::open(&fixture(n), None, || 1_700_000_000).expect("the fixture opens")
    }

    fn first_text(doc: &Document, page: usize) -> String {
        let blocks: Vec<serde_json::Value> = serde_json::from_str(&doc.text_blocks_json(page).expect("blocks")).expect("json");
        blocks.first().and_then(|b| b["text"].as_str()).unwrap_or_default().to_string()
    }

    #[test]
    fn opens_and_measures_pages() {
        let doc = open(3);
        assert_eq!(doc.page_count(), 3);
        assert_eq!(doc.page_size(0), Some((200.0, 300.0)));
        assert_eq!(doc.page_size(3), None);
        assert!(!doc.is_encrypted());
    }

    #[test]
    fn refuses_what_is_not_a_pdf() {
        let err = Document::open(b"hello", None, || 0).err().expect("not a PDF");
        assert_eq!(err.code(), "invalid");
        assert!(!err.message().is_empty());
    }

    #[test]
    fn renders_a_page_a_tile_and_refuses_bad_requests() {
        let mut doc = open(1);
        let page = doc.render(0, 2.0, None).expect("renders");
        assert_eq!((page.width, page.height), (400, 600));
        assert_eq!(page.rgba.len(), 400 * 600 * 4);
        assert!(page.rgba.chunks(4).any(|p| p[..3] != [255, 255, 255]), "the text is drawn");
        let tile = doc.render(0, 2.0, Some((0, 0, 100, 50))).expect("renders a tile");
        assert_eq!((tile.width, tile.height), (100, 50));
        assert!(doc.render(1, 1.0, None).is_err());
        assert!(doc.render(0, 0.0, None).is_err());
        assert!(doc.render(0, f32::NAN, None).is_err());
    }

    #[test]
    fn lists_and_rewrites_text_then_undoes_it() {
        let mut doc = open(1);
        assert_eq!(first_text(&doc, 0), "Page 1");
        assert!(doc.can_undo().is_none());
        doc.edit_text_block(0, 0, "Edited").expect("edits");
        assert_eq!(first_text(&doc, 0), "Edited");
        assert!(doc.can_undo().is_some());
        doc.undo().expect("undoes");
        assert_eq!(first_text(&doc, 0), "Page 1");
        assert!(doc.can_redo().is_some());
        doc.redo().expect("redoes");
        assert_eq!(first_text(&doc, 0), "Edited");
        assert!(doc.edit_text_block(0, 9, "x").is_err(), "no such block");
        assert!(doc.text_blocks_json(5).is_err(), "no such page");
    }

    #[test]
    fn changes_pages() {
        let mut doc = open(3);
        doc.move_pages(&[0], 2).expect("moves");
        assert_eq!(first_text(&doc, 2), "Page 1");
        doc.delete_pages(&[0]).expect("deletes");
        assert_eq!(doc.page_count(), 2);
        doc.rotate_pages(&[0], 90).expect("rotates");
        assert_eq!(doc.page_size(0), Some((300.0, 200.0)));
        doc.insert_blank_page(1, 100.0, 120.0).expect("inserts");
        assert_eq!(doc.page_count(), 3);
        assert!(doc.delete_pages(&[7]).is_err());
        assert!(doc.rotate_pages(&[0], 45).is_err(), "only quarter turns");
    }

    #[test]
    fn saved_bytes_reopen_with_the_edit_and_keep_the_original_as_a_prefix() {
        let original = fixture(1);
        let mut doc = Document::open(&original, None, || 1_700_000_000).expect("opens");
        doc.edit_text_block(0, 0, "Saved").expect("edits");
        let incremental = doc.save().expect("saves");
        assert!(incremental.starts_with(&original), "an incremental update appends to the file");
        let reopened = Document::open(&incremental, None, || 0).expect("reopens");
        assert_eq!(first_text(&reopened, 0), "Saved");
        let full = doc.save_full().expect("saves in full");
        assert_eq!(first_text(&Document::open(&full, None, || 0).expect("reopens"), 0), "Saved");
    }
}
