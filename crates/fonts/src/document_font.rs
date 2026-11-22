//! A full font program chosen by the user for newly added text.
//!
//! The bytes are the document input: they are validated against the project's open-licence and
//! embeddability policy here, kept in memory while the item is edited, and written beside the
//! item so reopening the PDF keeps its face. Displaying an interface fallback never creates one.

use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;
use std::path::Path;
use std::sync::Arc;

/// The largest full face accepted for a text item (16 MiB, as for an imported document font).
pub const MAX_BYTES: u64 = 16 << 20;

/// A validated full font program, cheap to clone and compare.
#[derive(Clone)]
pub struct DocumentFont {
    bytes: Arc<Vec<u8>>,
    family: String,
    face: String,
    /// Content identity: two loads of the same file compare equal without comparing every byte.
    key: u64,
}

impl DocumentFont {
    /// Validate and keep `bytes`. Only openly licensed, embeddable TrueType/OpenType faces are
    /// accepted: a system font without such a declaration never becomes PDF content.
    pub fn new(bytes: Vec<u8>) -> Result<Self, String> {
        Self::from_arc(Arc::new(bytes))
    }

    /// [`Self::new`] without copying bytes that are already shared.
    pub fn from_arc(bytes: Arc<Vec<u8>>) -> Result<Self, String> {
        if bytes.len() as u64 > MAX_BYTES {
            return Err(format!("the selected font exceeds {} MiB", MAX_BYTES >> 20));
        }
        crate::embedding::validate_document_input(bytes.as_slice())?;
        let (family, face) = crate::embedding::names(bytes.as_slice())?;
        let key = content_key(bytes.as_slice());
        Ok(Self { bytes, family, face, key })
    }

    /// Read and validate a font file. Missing or oversized files are refused before reading.
    pub fn from_path(path: &Path) -> Result<Self, String> {
        let meta = std::fs::metadata(path).map_err(|e| format!("couldn't read the font: {e}"))?;
        if !meta.is_file() {
            return Err("the selected path is not a font file".into());
        }
        if meta.len() > MAX_BYTES {
            return Err(format!("the selected font exceeds {} MiB", MAX_BYTES >> 20));
        }
        let bytes = std::fs::read(path).map_err(|e| format!("couldn't read the font: {e}"))?;
        Self::new(bytes)
    }

    /// The bundled Inter Regular face, always available for Cyrillic, Greek and extra Latin
    /// letters. `None` only if the bundled program itself fails the document-input checks.
    pub fn inter() -> Option<Self> {
        static CACHE: std::sync::OnceLock<Option<DocumentFont>> = std::sync::OnceLock::new();
        CACHE.get_or_init(|| Self::new(crate::INTER_REGULAR.to_vec()).ok()).clone()
    }

    pub fn bytes(&self) -> &[u8] {
        self.bytes.as_slice()
    }

    pub fn family(&self) -> &str {
        &self.family
    }

    pub fn face(&self) -> &str {
        &self.face
    }

    /// Content identity of the program, used by equality and by a library's duplicate check.
    pub fn key(&self) -> u64 {
        self.key
    }
}

impl std::fmt::Debug for DocumentFont {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocumentFont").field("family", &self.family).field("face", &self.face).field("key", &self.key).finish()
    }
}

impl PartialEq for DocumentFont {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key && self.family == other.family && self.face == other.face
    }
}

impl Eq for DocumentFont {}

fn content_key(bytes: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    h.write(bytes);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_face_is_a_document_font_and_content_equal_after_a_copy() {
        let Ok(font) = DocumentFont::new(crate::INTER_REGULAR.to_vec()) else {
            eprintln!("the bundled Inter face does not pass the document-font policy");
            return;
        };
        assert_eq!(font.family(), "Inter");
        let copied = DocumentFont::new(font.bytes().to_vec());
        assert!(copied.is_ok_and(|copy| copy == font), "the same bytes are the same font");
    }
}
