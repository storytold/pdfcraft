//! The 3D models on a page (ISO 32000-2 §13.6), read so a renderer can draw them.

pub use pdfcraft_annot::{MAX_MODEL_BYTES, Model3d, Model3dKind};

use crate::Document;

impl Document {
    /// The `/Annots` positions of the 3D annotations on 0-based `page`, in order.
    pub fn model_3d_annotations(&self, page: usize) -> Result<Vec<usize>, String> {
        let editor = self.editor.as_ref().ok_or("the document can't be read")?;
        pdfcraft_annot::model_3d_annotations(&editor.cos, page).map_err(|e| e.to_string())
    }

    /// The decoded model of the 3D annotation at `/Annots` position `index` on 0-based `page`.
    pub fn model_3d(&self, page: usize, index: usize) -> Result<Model3d, String> {
        let editor = self.editor.as_ref().ok_or("the document can't be read")?;
        pdfcraft_annot::model_3d(&editor.cos, page, index).map_err(|e| e.to_string())
    }
}
