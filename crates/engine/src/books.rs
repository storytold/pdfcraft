//! Open an EPUB as a PDF (#493, #2620): the book is laid out by `pdfcraft_create::from_epub` on
//! the paper the session was given (the region's default, set by the frontend), checked like any
//! new file, and what the conversion left out or chose goes into the new document's repair log.

use std::sync::Arc;

use crate::{DocId, EditError, OpenContext, Session};

/// A converted book: its PDF and the notes for the repair log.
pub type Book = (Arc<Vec<u8>>, Vec<String>);

fn title_of(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(s, _)| s)
}

/// Convert, then write and check the new file (`now` dates it).
fn convert(name: &str, bytes: &[u8], paper: (f64, f64), now: Option<i64>) -> Result<Book, EditError> {
    let converted = crate::guard(|| pdfcraft_create::from_epub(bytes, paper, title_of(name)))
        .map_err(|_| EditError::Source(format!("{name}: the book could not be read")))?
        .map_err(|e| EditError::Source(format!("{name}: {e}")))?;
    let bytes = crate::write_checked(&converted.document, now)?;
    Ok((bytes, converted.notes))
}

impl Session {
    /// The paper reflowed books are set on (US Letter until a frontend says otherwise).
    pub fn book_paper(&self) -> (f64, f64) {
        self.book_paper.unwrap_or(pdfcraft_create::LETTER)
    }

    /// Set the paper reflowed books are set on, in points (the frontend passes its region's,
    /// [`pdfcraft_create::paper_for_region`]).
    pub fn set_book_paper(&mut self, paper: (f64, f64)) {
        self.book_paper = Some(paper);
    }

    /// An EPUB as PDF bytes (not opened), and the notes for its repair log.
    pub fn create_from_epub(&self, name: &str, bytes: &[u8]) -> Result<Book, EditError> {
        self.create_from_epub_on(name, bytes, self.book_paper())
    }

    /// [`Self::create_from_epub`] with reflowed text set on `paper` (points).
    pub fn create_from_epub_on(&self, name: &str, bytes: &[u8], paper: (f64, f64)) -> Result<Book, EditError> {
        convert(name, bytes, paper, self.now())
    }

    /// Mark document `id` as converted from a book and put the conversion's `notes` in its repair
    /// log (Document Properties ▸ Advanced).
    pub fn note_book(&mut self, id: DocId, notes: &[String]) {
        if let Some(d) = self.docs.iter_mut().find(|d| d.id == id) {
            d.from_book = true;
            if let Some(e) = d.editor.as_mut() {
                for n in notes {
                    e.cos.note_repair(n.clone());
                }
            }
        }
    }
}

impl OpenContext {
    /// [`Session::create_from_epub`] away from the session (an open worker).
    pub fn convert_book(&self, name: &str, bytes: &[u8]) -> Result<Book, EditError> {
        convert(name, bytes, self.book_paper, self.now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-chapter book: container, package, a chapter with a heading and two paragraphs.
    pub(crate) fn tiny_book() -> Vec<u8> {
        fn entry(out: &mut Vec<u8>, dir: &mut Vec<u8>, name: &str, data: &[u8]) {
            let offset = out.len() as u32;
            let crc = {
                let mut c = 0xFFFF_FFFFu32;
                for &b in data {
                    c ^= u32::from(b);
                    for _ in 0..8 {
                        c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
                    }
                }
                !c
            };
            let mut head = Vec::new();
            head.extend_from_slice(&20u16.to_le_bytes());
            head.extend_from_slice(&0u16.to_le_bytes());
            head.extend_from_slice(&0u16.to_le_bytes());
            head.extend_from_slice(&[0; 4]);
            head.extend_from_slice(&crc.to_le_bytes());
            head.extend_from_slice(&(data.len() as u32).to_le_bytes());
            head.extend_from_slice(&(data.len() as u32).to_le_bytes());
            head.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(b"PK\x03\x04");
            out.extend_from_slice(&head);
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);
            dir.extend_from_slice(b"PK\x01\x02");
            dir.extend_from_slice(&20u16.to_le_bytes());
            dir.extend_from_slice(&head);
            dir.extend_from_slice(&[0; 12]);
            dir.extend_from_slice(&offset.to_le_bytes());
            dir.extend_from_slice(name.as_bytes());
        }
        let files: [(&str, &str); 4] = [
            ("mimetype", "application/epub+zip"),
            (
                "META-INF/container.xml",
                r#"<?xml version="1.0"?><container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0"><rootfiles><rootfile full-path="c.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
            ),
            (
                "c.opf",
                r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="i"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="i">urn:uuid:0</dc:identifier><dc:title>Tiny Book</dc:title><dc:language>en</dc:language></metadata><manifest><item id="a" href="a.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="a"/></spine></package>"#,
            ),
            (
                "a.xhtml",
                r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><head><title>a</title></head><body><h1>First Chapter</h1><p>It was a bright cold day.</p><p>Second paragraph &amp; more.</p></body></html>"#,
            ),
        ];
        let (mut out, mut dir) = (Vec::new(), Vec::new());
        for (n, d) in files {
            entry(&mut out, &mut dir, n, d.as_bytes());
        }
        let start = out.len() as u32;
        out.extend_from_slice(&dir);
        out.extend_from_slice(b"PK\x05\x06\0\0\0\0");
        out.extend_from_slice(&4u16.to_le_bytes());
        out.extend_from_slice(&4u16.to_le_bytes());
        out.extend_from_slice(&(dir.len() as u32).to_le_bytes());
        out.extend_from_slice(&start.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    #[test]
    fn a_book_opens_with_its_text_title_and_notes() {
        let mut s = Session::new();
        s.set_book_paper(pdfcraft_create::A4);
        let book = tiny_book();
        assert_eq!(pdfcraft_create::source_kind("tiny.epub", &book), Some(pdfcraft_create::SourceKind::Epub));
        let (pdf, notes) = s.create_from_epub("tiny.epub", &book).unwrap();
        assert!(notes[0].contains("A4"), "{notes:?}");
        let id = s.open_new("tiny.pdf", pdf).unwrap();
        s.note_book(id, &notes);
        let doc = s.get(id).unwrap();
        assert!(doc.from_book);
        assert_eq!(doc.info.title.as_deref(), Some("Tiny Book"));
        assert!((doc.info.pages[0].width - 595.276).abs() < 0.01);
        assert!(doc.repair_log().iter().any(|l| l.contains("A4")));
        let words: Vec<String> = doc.words().into_iter().map(|w| w.text).collect();
        for w in ["First", "Chapter", "bright", "cold", "&", "more."] {
            assert!(words.iter().any(|x| x == w), "{w} in {words:?}");
        }
        // Through the general conversion too (Combine, Create multiple, batch).
        let (kind, _) = s.convert_to_pdf("tiny.epub", &Arc::new(book)).unwrap();
        assert_eq!(kind, pdfcraft_create::SourceKind::Epub);
    }

    #[test]
    fn a_broken_book_says_why() {
        let s = Session::new();
        let mut book = tiny_book();
        // Damage the package document's name in the container.
        let at = book.windows(9).position(|w| w == b"full-path").unwrap();
        book[at + 11] = b'X';
        let err = s.create_from_epub("bad.epub", &book).unwrap_err().to_string();
        assert!(err.contains("bad.epub") && err.contains("package document"), "{err}");
    }
}
