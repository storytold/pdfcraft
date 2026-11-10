//! Export a PDF ▸ EPUB (#493, #2620, #2628): the document as a reflowable book, or a page per PDF
//! page (fixed layout). What the book is made of comes from the document here ([`EpubBook`]); the
//! fixed layout's page pictures are drawn later, on whatever thread exports, from an
//! [`crate::export::ExportSource`].

use pdfcraft_export::{EpubMeta, EpubTags, FixedPage, Page, TocEntry};
use pdfcraft_render::OutlineItem;
use sha2::{Digest, Sha256};

use crate::export::{Exporter, ImageFormat};

/// How the book lays its pages out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EpubLayout {
    /// Text that reflows to the reader's screen and font size, in chapters.
    #[default]
    Reflowable,
    /// Each PDF page as it looks, its text laid over the picture so it can be searched.
    Fixed,
}

/// The resolution a fixed-layout book's page pictures are drawn at.
pub const FIXED_DPI: f64 = 150.0;
/// Their JPEG quality.
const FIXED_QUALITY: u8 = 85;
/// The most the pictures of a fixed-layout book may come to together: the book is built in memory
/// (the pictures, then the archive holding them), so a very large document is refused with a
/// message, not left to run the machine out of memory.
pub const FIXED_MAX_PICTURE_BYTES: usize = 400 << 20;

/// Everything a book is made from, read from the document. It holds no reference to it, so it can
/// go to another thread.
#[derive(Clone, Debug)]
pub struct EpubBook {
    pub pages: Vec<Page>,
    pub title: String,
    pub author: Option<String>,
    /// The catalog's `/Lang`, `und` when there is none.
    pub language: String,
    /// `urn:uuid:` from the document's bytes: the same file gives the same identifier.
    pub identifier: String,
    /// The bookmarks; a tagged document without them gets its headings.
    pub toc: Vec<TocEntry>,
    pub tags: EpubTags,
}

impl EpubBook {
    fn meta<'a>(&'a self, modified: &'a str) -> EpubMeta<'a> {
        EpubMeta { title: &self.title, author: self.author.as_deref(), language: &self.language, identifier: &self.identifier, modified }
    }

    /// The reflowable book; `modified` is Unix time (`dcterms:modified`).
    pub fn reflowable(&self, modified: i64) -> Vec<u8> {
        let modified = pdfcraft_export::iso_utc(modified);
        pdfcraft_export::epub(&self.pages, &self.meta(&modified), &self.toc, &self.tags)
    }

    /// The fixed-layout book, its pictures drawn by `exporter` (from the same document); `progress`
    /// hears each page as it is drawn. Refused when the pictures come to more than
    /// [`FIXED_MAX_PICTURE_BYTES`].
    pub fn fixed(&self, exporter: &mut Exporter, modified: i64, progress: impl FnMut(usize)) -> Result<Vec<u8>, String> {
        self.fixed_within(exporter, modified, FIXED_MAX_PICTURE_BYTES, progress)
    }

    fn fixed_within(&self, exporter: &mut Exporter, modified: i64, limit: usize, mut progress: impl FnMut(usize)) -> Result<Vec<u8>, String> {
        let mut pages = Vec::with_capacity(self.pages.len());
        let mut held = 0usize;
        for (i, page) in self.pages.iter().enumerate() {
            progress(i);
            let picture = exporter.image(i, FIXED_DPI, ImageFormat::Jpeg { quality: FIXED_QUALITY })?;
            held = held.saturating_add(picture.len());
            if held > limit {
                return Err(format!(
                    "page {}: the page pictures come to more than {} MB, the most a fixed-layout book holds in memory; export as reflowable instead",
                    i + 1,
                    limit >> 20
                ));
            }
            pages.push(FixedPage { width: page.width, height: page.height, ext: "jpg", picture, page: page.clone() });
        }
        let modified = pdfcraft_export::iso_utc(modified);
        Ok(pdfcraft_export::epub_fixed(&pages, &self.meta(&modified), &self.toc))
    }
}

/// The bookmarks as table-of-contents entries, depth first; those without a page are left out
/// and their children move up a level.
fn outline_entries(items: &[OutlineItem], level: u8, out: &mut Vec<TocEntry>) {
    for item in items {
        let title = item.title.trim();
        match item.page {
            Some(page) if !title.is_empty() => {
                out.push(TocEntry { title: title.to_string(), level, page });
                outline_entries(&item.children, level.saturating_add(1).min(6), out);
            }
            _ => outline_entries(&item.children, level, out),
        }
    }
}

/// The grandfathered tags of RFC 5646 §2.2.8, irregular and regular, lowercase: they are tags
/// though their shape is not the grammar's.
const GRANDFATHERED: [&str; 26] = [
    "en-gb-oed",
    "i-ami",
    "i-bnn",
    "i-default",
    "i-enochian",
    "i-hak",
    "i-klingon",
    "i-lux",
    "i-mingo",
    "i-navajo",
    "i-pwn",
    "i-tao",
    "i-tay",
    "i-tsu",
    "sgn-be-fr",
    "sgn-be-nl",
    "sgn-ch-de",
    "art-lojban",
    "cel-gaulish",
    "no-bok",
    "no-nyn",
    "zh-guoyu",
    "zh-hakka",
    "zh-min",
    "zh-min-nan",
    "zh-xiang",
];

/// Whether `tag` is a well-formed BCP 47 language tag (RFC 5646 §2.1, the syntax only: whether the
/// subtags are registered is not asked): a language with its extended languages, a script, a
/// region, variants, extensions (a singleton then its subtags), a private use part, or a private
/// use tag alone, or a grandfathered tag.
fn language_tag(tag: &str) -> bool {
    let lower = tag.to_ascii_lowercase();
    if GRANDFATHERED.contains(&lower.as_str()) {
        return true;
    }
    let t: Vec<&str> = lower.split('-').collect();
    if t.iter().any(|s| s.is_empty() || s.len() > 8 || !s.bytes().all(|c| c.is_ascii_alphanumeric())) {
        return false;
    }
    let alpha = |s: &str| s.bytes().all(|c| c.is_ascii_alphabetic());
    let digit = |s: &str| s.bytes().all(|c| c.is_ascii_digit());
    // Private use: `x` then 1 to 8 characters each.
    let private_from = |i: usize| t.len() > i + 1 && t[i] == "x";
    if t[0] == "x" {
        return private_from(0);
    }
    if !(2..=8).contains(&t[0].len()) || !alpha(t[0]) {
        return false;
    }
    let mut i = 1;
    // Extended languages go only with a language of 2 or 3 letters.
    if t[0].len() <= 3 {
        let mut ext = 0;
        while ext < 3 && i < t.len() && t[i].len() == 3 && alpha(t[i]) {
            (i, ext) = (i + 1, ext + 1);
        }
    }
    if i < t.len() && t[i].len() == 4 && alpha(t[i]) {
        i += 1;
    }
    if i < t.len() && ((t[i].len() == 2 && alpha(t[i])) || (t[i].len() == 3 && digit(t[i]))) {
        i += 1;
    }
    while i < t.len() && ((5..=8).contains(&t[i].len()) || (t[i].len() == 4 && t[i].as_bytes()[0].is_ascii_digit())) {
        i += 1;
    }
    while i < t.len() && t[i].len() == 1 && t[i] != "x" {
        let from = i + 1;
        i = from;
        while i < t.len() && t[i].len() >= 2 {
            i += 1;
        }
        if i == from {
            return false;
        }
    }
    if i < t.len() && t[i] == "x" {
        return private_from(i);
    }
    i == t.len()
}

/// What [`crate::Document::epub_book`] reads from a document, owned (the PDF's objects are shared,
/// not copied), so the book can be made on another thread, after a place for it is chosen, and not
/// while the window waits.
#[derive(Clone)]
pub struct EpubSource {
    name: String,
    bytes: std::sync::Arc<Vec<u8>>,
    info: pdfcraft_render::DocInfo,
    cos: Option<pdfcraft_cos::Document>,
}

impl EpubSource {
    /// What an EPUB of the document is made of: every page's paragraphs, tables and images, its
    /// title, author and language, the bookmarks, and a tagged document's heading levels and figure
    /// descriptions.
    pub fn book(&self) -> EpubBook {
        let cos = self.cos.as_ref();
        let pages = match cos {
            Some(cos) => {
                crate::compare::export_pages_of(
                    &self.info,
                    cos,
                    true,
                    crate::compare::MAX_EXPORT_IMAGE_BYTES,
                    &mut pdfcraft_cos::ImageBudget::new(cos),
                )
                .0
            }
            None => Vec::new(),
        };
        let title = self
            .info
            .title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| self.name.trim_end_matches(".pdf").trim_end_matches(".PDF").to_string());
        let author = self.info.author.clone().filter(|a| !a.trim().is_empty());
        let mut toc = Vec::new();
        outline_entries(&self.info.outline, 1, &mut toc);
        let mut tags = EpubTags::default();
        let mut language = None;
        if let Some(cos) = cos {
            language = cos
                .root()
                .and_then(|r| cos.get(r).as_dict().cloned())
                .and_then(|cat| cat.get(b"Lang").map(|o| cos.resolve(o)).and_then(|o| o.as_string().map(|s| s.to_text())))
                .map(|s| s.trim().to_string())
                .filter(|s| language_tag(s));
            if self.info.tagged {
                tags.headings = pdfcraft_a11y::headings(cos)
                    .into_iter()
                    .filter(|h| h.page < pages.len())
                    .map(|h| (h.page, h.level.clamp(1, 6), h.title))
                    .collect();
                tags.alts = pdfcraft_a11y::figures(cos)
                    .into_iter()
                    .filter_map(|f| {
                        let (page, bbox, alt) = (f.page?, f.bbox?, f.alt?);
                        let info = self.info.pages.get(page)?;
                        // Figures are found by where they are on the displayed page, as the
                        // exported images are (points, y up).
                        let a = info.user_to_view(bbox[0] as f32, bbox[1] as f32);
                        let b = info.user_to_view(bbox[2] as f32, bbox[3] as f32);
                        let rect = [
                            a[0].min(b[0]) as f64,
                            (info.height - a[1].max(b[1])) as f64,
                            a[0].max(b[0]) as f64,
                            (info.height - a[1].min(b[1])) as f64,
                        ];
                        (!alt.trim().is_empty()).then_some((page, rect, alt))
                    })
                    .collect();
                if toc.is_empty() {
                    toc = tags.headings.iter().map(|(page, level, title)| TocEntry { title: title.clone(), level: *level, page: *page }).collect();
                }
            }
        }
        let identifier = pdfcraft_export::urn_uuid(&Sha256::digest(self.bytes.as_slice()));
        EpubBook { pages, title, author, language: language.unwrap_or_else(|| "und".into()), identifier, toc, tags }
    }
}

impl crate::Document {
    /// What [`Self::epub_book`] reads, to make the book elsewhere.
    pub fn epub_source(&self) -> EpubSource {
        EpubSource { name: self.name.clone(), bytes: self.bytes.clone(), info: self.info.clone(), cos: self.editor.as_ref().map(|e| e.cos.clone()) }
    }

    /// What an EPUB of this document is made of (Export ▸ EPUB).
    pub fn epub_book(&self) -> EpubBook {
        self.epub_source().book()
    }

    /// The document as an EPUB file laid out as `layout`, dated `modified` (Unix time).
    pub fn export_epub(&self, layout: EpubLayout, modified: i64) -> Result<Vec<u8>, String> {
        let book = self.epub_book();
        match layout {
            EpubLayout::Reflowable => Ok(book.reflowable(modified)),
            EpubLayout::Fixed => book.fixed(&mut Exporter::new(self), modified, |_| {}),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn item(title: &str, page: Option<usize>, children: Vec<OutlineItem>) -> OutlineItem {
        OutlineItem {
            title: title.into(),
            page,
            view: Default::default(),
            children,
            open: false,
            color: None,
            italic: false,
            bold: false,
            action: None,
        }
    }

    #[test]
    fn bookmarks_become_the_contents_and_pageless_ones_lift_their_children() {
        let outline = vec![
            item("Part one", Some(0), vec![item("Chapter 1", Some(1), vec![]), item("  ", Some(2), vec![])]),
            item("Appendices", None, vec![item("A", Some(5), vec![])]),
        ];
        let mut out = Vec::new();
        outline_entries(&outline, 1, &mut out);
        let got: Vec<(&str, u8, usize)> = out.iter().map(|e| (e.title.as_str(), e.level, e.page)).collect();
        assert_eq!(got, [("Part one", 1, 0), ("Chapter 1", 2, 1), ("A", 1, 5)]);
    }

    #[test]
    fn a_document_exports_as_both_layouts_with_its_title_and_a_stable_identifier() {
        let mut session = crate::Session::new().with_clock(|| 1_792_108_800);
        let bytes = session.create_blank(200.0, 300.0, 3).unwrap();
        let id = session.open("Report.pdf".to_string(), None, bytes, None).unwrap();
        session.apply(id, crate::Edit::SetInfo { key: "Title".into(), value: "Quarterly report".into() }).unwrap();
        let doc = session.get(id).unwrap();
        let book = doc.epub_book();
        assert_eq!(book.title, "Quarterly report");
        assert_eq!(book.language, "und");
        assert_eq!(book.pages.len(), 3);
        assert!(book.identifier.starts_with("urn:uuid:"));
        assert_eq!(book.identifier, doc.epub_book().identifier, "the same document, the same identifier");
        for layout in [EpubLayout::Reflowable, EpubLayout::Fixed] {
            let file = doc.export_epub(layout, session.now_secs()).unwrap();
            assert!(file.starts_with(b"PK\x03\x04") && file[30..].starts_with(b"mimetypeapplication/epub+zip"), "{layout:?}");
        }
        // Pictures past the limit refuse the fixed layout, saying what to do instead.
        let err = book.fixed_within(&mut Exporter::new(doc), session.now_secs(), 1, |_| {}).unwrap_err();
        assert!(err.contains("page 1") && err.contains("reflowable"), "{err}");
        assert!(book.fixed_within(&mut Exporter::new(doc), session.now_secs(), usize::MAX, |_| {}).is_ok());
    }
}

#[cfg(test)]
mod language_tests;
