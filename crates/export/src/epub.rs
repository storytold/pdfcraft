//! EPUB 3 export (#493, #2620, #2628): a book reading systems open and reflow, or one page per PDF
//! page for documents that don't reflow.
//!
//! Reflowable: the same paragraphs, tables and images the HTML and Word writers use, in chapters
//! that start at each top-level heading (every [`PAGES_PER_CHAPTER`] pages when there is none), with
//! a page-list so the PDF's page numbers can still be followed. A tagged PDF gives its heading
//! levels and figure alternate text ([`Tags`]); otherwise headings come from the type size, as in
//! the HTML export.
//!
//! Fixed layout: each page as a picture with its text laid over it, invisible, so it can be
//! searched and selected (`rendition:layout` pre-paginated, the viewport the page's size).
//!
//! Both are packaged as EPUB 3 requires: `mimetype` first and stored, `META-INF/container.xml`,
//! the package document, a navigation document, and `toc.ncx` for EPUB 2 reading systems.

use crate::zip::Zip;
use crate::{Image, Item, Page, Table, body_size, esc, items, level};

/// Chapters of an untagged document without headings: this many pages each.
pub const PAGES_PER_CHAPTER: usize = 10;

/// What the package document says about the book.
#[derive(Clone, Debug)]
pub struct Meta<'a> {
    pub title: &'a str,
    pub author: Option<&'a str>,
    /// BCP 47 (`und` when unknown).
    pub language: &'a str,
    /// A URN that stays the same for the same document (`urn:uuid:…`).
    pub identifier: &'a str,
    /// `dcterms:modified`, `CCYY-MM-DDThh:mm:ssZ` ([`iso_utc`]).
    pub modified: &'a str,
}

/// A table-of-contents entry: a bookmark, or a heading. `level` 1 is the top.
#[derive(Clone, Debug, PartialEq)]
pub struct TocEntry {
    pub title: String,
    pub level: u8,
    /// 0-based page.
    pub page: usize,
}

/// What a tagged PDF says about its structure: heading levels and figure descriptions. Empty for an
/// untagged document.
#[derive(Clone, Debug, Default)]
pub struct Tags {
    /// (0-based page, level 1–6, the heading's text).
    pub headings: Vec<(usize, u8, String)>,
    /// (0-based page, where the figure is on the displayed page (y up), its alternate text).
    pub alts: Vec<(usize, [f64; 4], String)>,
}

/// A page of a fixed-layout book: its picture and the text on it.
#[derive(Clone, Debug)]
pub struct FixedPage {
    pub width: f64,
    pub height: f64,
    /// "png" or "jpg".
    pub ext: &'static str,
    pub picture: Vec<u8>,
    pub page: Page,
}

/// `dcterms:modified` for Unix time `secs` (UTC). A time chrono cannot represent becomes the Unix
/// epoch, a valid date, so the package metadata stays well-formed.
pub fn iso_utc(secs: i64) -> String {
    let when = chrono::DateTime::from_timestamp(secs, 0).unwrap_or(chrono::DateTime::UNIX_EPOCH);
    when.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// A `urn:uuid:` built from `digest` (at least 16 bytes), marked as a name-based UUID, so the same
/// document gets the same identifier.
pub fn urn_uuid(digest: &[u8]) -> String {
    let mut b = [0u8; 16];
    for (i, x) in digest.iter().take(16).enumerate() {
        b[i] = *x;
    }
    b[6] = (b[6] & 0x0f) | 0x50;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("urn:uuid:{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

const CONTAINER: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>
"#;

const STYLE: &str = "body { margin: 0 5%; line-height: 1.4; }\nh1, h2, h3, h4, h5, h6 { line-height: 1.2; }\nfigure { margin: 1em 0; text-align: center; }\nimg { max-width: 100%; height: auto; }\ntable { border-collapse: collapse; margin: 1em 0; }\ntd, th { border: 1px solid #999; padding: 0.2em 0.4em; vertical-align: top; }\n";

fn media_type(ext: &str) -> &'static str {
    if ext == "jpg" { "image/jpeg" } else { "image/png" }
}

/// An XHTML content document around `body`.
fn xhtml(title: &str, lang: &str, head_extra: &str, body: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE html>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\" lang=\"{l}\" xml:lang=\"{l}\">\n<head>\n<meta charset=\"UTF-8\"/>\n<title>{t}</title>\n{head_extra}</head>\n<body>\n{body}</body>\n</html>\n",
        l = esc(lang),
        t = esc(title)
    )
}

/// A page-number marker (EPUB's pagebreak) for 0-based page `p`.
fn page_marker(p: usize) -> String {
    format!("<span epub:type=\"pagebreak\" role=\"doc-pagebreak\" id=\"page-{n}\" aria-label=\"{n}\"></span>\n", n = p + 1)
}

/// Whitespace folded, for matching a block's text with a tagged heading's.
fn folded(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn overlap(a: [f64; 4], b: [f64; 4]) -> f64 {
    let w = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let h = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    w * h
}

fn table_html(t: &Table) -> String {
    let mut s = String::from("<table>\n");
    for row in &t.rows {
        s.push_str("<tr>");
        for c in row {
            let span = if c.span > 1 { format!(" colspan=\"{}\"", c.span) } else { String::new() };
            let mut text = esc(&c.text);
            if c.bold {
                text = format!("<strong>{text}</strong>");
            }
            if c.italic {
                text = format!("<em>{text}</em>");
            }
            s.push_str(&format!("<td{span}>{text}</td>"));
        }
        s.push_str("</tr>\n");
    }
    s.push_str("</table>\n");
    s
}

/// The reflowable book.
pub fn epub(pages: &[Page], meta: &Meta, toc: &[TocEntry], tags: &Tags) -> Vec<u8> {
    let tagged = !tags.headings.is_empty();
    let body = body_size(pages);
    // Chapters: (first page, XHTML body). A chapter starts at each top-level heading.
    let mut chapters: Vec<(usize, String)> = Vec::new();
    let mut images: Vec<(String, &Image)> = Vec::new();
    let mut headings: Vec<TocEntry> = Vec::new();
    let mut page = 0usize;
    let mut current = page_marker(0);
    let mut first_page = 0usize;
    let mut pages_in_chapter = 1usize;
    // The chapter each page's marker ended up in, and the one each heading is in: a marker can
    // stay behind in the chapter before a heading that starts mid-page.
    let mut marker_chapter: Vec<usize> = vec![0];
    let mut heading_chapter: Vec<usize> = Vec::new();
    let has_top = |p: &[Page]| -> bool {
        if tagged { tags.headings.iter().any(|h| h.1 == 1) } else { p.iter().flat_map(|pg| &pg.blocks).any(|b| level(b, body) == 1) }
    };
    let split_at_headings = has_top(pages);
    for item in items(pages) {
        match item {
            Item::PageBreak => {
                page += 1;
                pages_in_chapter += 1;
                if !split_at_headings && pages_in_chapter > PAGES_PER_CHAPTER {
                    chapters.push((first_page, std::mem::take(&mut current)));
                    first_page = page;
                    pages_in_chapter = 1;
                }
                current.push_str(&page_marker(page));
                marker_chapter.push(chapters.len());
            }
            Item::Para(b, guessed) => {
                let lvl = if tagged {
                    let text = folded(&b.iter().map(|r| r.text.as_str()).collect::<String>());
                    tags.headings.iter().find(|h| h.0 == page && folded(&h.2) == text).map_or(0, |h| h.1.clamp(1, 6))
                } else {
                    guessed
                };
                if lvl == 1 && split_at_headings && !current.trim().is_empty() && current.trim() != page_marker(page).trim() {
                    // The marker for this page belongs with the new chapter.
                    let marker = page_marker(page);
                    let had_marker = current.ends_with(&marker);
                    if had_marker {
                        current.truncate(current.len() - marker.len());
                    }
                    chapters.push((first_page, std::mem::take(&mut current)));
                    first_page = page;
                    if had_marker {
                        current.push_str(&marker);
                        if let Some(c) = marker_chapter.get_mut(page) {
                            *c = chapters.len();
                        }
                    }
                }
                let plain: String = b.iter().map(|r| r.text.as_str()).collect();
                let text = if lvl == 0 {
                    // Each run keeps its own bold and italic.
                    let last = b.len().saturating_sub(1);
                    let mut t = String::new();
                    for (i, r) in b.iter().enumerate() {
                        let mut s = r.text.as_str();
                        if i == 0 {
                            s = s.trim_start();
                        }
                        if i == last {
                            s = s.trim_end();
                        }
                        let mut run = esc(s);
                        if r.block.bold {
                            run = format!("<strong>{run}</strong>");
                        }
                        if r.block.italic {
                            run = format!("<em>{run}</em>");
                        }
                        t.push_str(&run);
                    }
                    t
                } else {
                    esc(plain.trim())
                };
                if lvl == 0 {
                    current.push_str(&format!("<p>{text}</p>\n"));
                } else {
                    let id = format!("h-{}", headings.len() + 1);
                    current.push_str(&format!("<h{lvl} id=\"{id}\">{text}</h{lvl}>\n"));
                    heading_chapter.push(chapters.len());
                    headings.push(TocEntry { title: plain.trim().to_string(), level: lvl, page });
                }
            }
            Item::Table(t) => current.push_str(&table_html(&t)),
            Item::Img(im) => {
                let name = format!("images/img-{}.{}", images.len() + 1, im.ext);
                let alt = tags
                    .alts
                    .iter()
                    .filter(|a| a.0 == page && overlap(a.1, im.rect) > 0.0)
                    .max_by(|a, b| overlap(a.1, im.rect).total_cmp(&overlap(b.1, im.rect)))
                    .map_or(String::new(), |a| a.2.clone());
                current.push_str(&format!("<figure><img src=\"{}\" alt=\"{}\"/></figure>\n", esc(&name), esc(&alt)));
                images.push((name, im));
            }
        }
    }
    chapters.push((first_page, current));
    let chapter_of = |p: usize| chapters.iter().rposition(|(start, _)| *start <= p).unwrap_or(0);
    let heading_entries = toc.is_empty() && !headings.is_empty();
    let entries: Vec<TocEntry> = if !toc.is_empty() {
        toc.to_vec()
    } else if !headings.is_empty() {
        headings
    } else {
        chapters.iter().map(|(p, _)| TocEntry { title: format!("{} {}", meta.title, p + 1), level: 1, page: *p }).collect()
    };
    let target = |p: usize| format!("chapter-{}.xhtml#page-{}", marker_chapter.get(p).copied().unwrap_or_else(|| chapter_of(p)) + 1, p + 1);
    // A heading entry goes to the heading itself, any other to its page's marker.
    let entry_target = |i: usize, e: &TocEntry| match heading_chapter.get(i) {
        Some(c) if heading_entries => format!("chapter-{}.xhtml#h-{}", c + 1, i + 1),
        _ => target(e.page),
    };

    let mut zip = Zip::default();
    zip.add("mimetype", b"application/epub+zip", false);
    zip.add("META-INF/container.xml", CONTAINER.as_bytes(), true);
    zip.add("OEBPS/style.css", STYLE.as_bytes(), true);
    let link = "<link rel=\"stylesheet\" type=\"text/css\" href=\"style.css\"/>\n";
    for (i, (_, body)) in chapters.iter().enumerate() {
        zip.add(&format!("OEBPS/chapter-{}.xhtml", i + 1), xhtml(meta.title, meta.language, link, body).as_bytes(), true);
    }
    for (name, im) in &images {
        zip.add(&format!("OEBPS/{name}"), &im.bytes, false);
    }
    let page_list: Vec<(String, String)> = (0..pages.len()).map(|p| ((p + 1).to_string(), target(p))).collect();
    zip.add("OEBPS/nav.xhtml", nav(meta, &entries, &page_list, &entry_target).as_bytes(), true);
    zip.add("OEBPS/toc.ncx", ncx(meta, &entries, &entry_target).as_bytes(), true);
    let mut manifest = vec![
        ("nav".to_string(), "nav.xhtml".to_string(), "application/xhtml+xml", Some("nav")),
        ("ncx".to_string(), "toc.ncx".to_string(), "application/x-dtbncx+xml", None),
        ("css".to_string(), "style.css".to_string(), "text/css", None),
    ];
    let mut spine = Vec::new();
    for i in 0..chapters.len() {
        manifest.push((format!("chapter-{}", i + 1), format!("chapter-{}.xhtml", i + 1), "application/xhtml+xml", None));
        spine.push(format!("chapter-{}", i + 1));
    }
    for (i, (name, im)) in images.iter().enumerate() {
        manifest.push((format!("img-{}", i + 1), name.clone(), media_type(im.ext), None));
    }
    zip.add("OEBPS/content.opf", opf(meta, &manifest, &spine, false).as_bytes(), true);
    zip.finish()
}

/// The fixed-layout book: a page per PDF page.
pub fn epub_fixed(pages: &[FixedPage], meta: &Meta, toc: &[TocEntry]) -> Vec<u8> {
    let mut zip = Zip::default();
    zip.add("mimetype", b"application/epub+zip", false);
    zip.add("META-INF/container.xml", CONTAINER.as_bytes(), true);
    let mut manifest = vec![
        ("nav".to_string(), "nav.xhtml".to_string(), "application/xhtml+xml", Some("nav")),
        ("ncx".to_string(), "toc.ncx".to_string(), "application/x-dtbncx+xml", None),
    ];
    let mut spine = Vec::new();
    for (i, p) in pages.iter().enumerate() {
        let (w, h) = (p.width.round().max(1.0), p.height.round().max(1.0));
        let picture = format!("images/page-{}.{}", i + 1, p.ext);
        let mut body = format!(
            "<div style=\"position: relative; width: {w}px; height: {h}px; overflow: hidden;\">\n<img src=\"{}\" alt=\"\" style=\"position: absolute; left: 0; top: 0; width: {w}px; height: {h}px;\"/>\n",
            esc(&picture)
        );
        for b in p.page.blocks.iter().filter(|b| !b.text.trim().is_empty()) {
            // The text sits where it is on the page, invisible: the picture shows it, the text is
            // there to search, select and read aloud.
            let (x, top) = (b.rect[0], p.height - b.rect[3]);
            body.push_str(&format!(
                "<p style=\"position: absolute; left: {x:.1}px; top: {top:.1}px; width: {:.1}px; margin: 0; font-size: {:.1}px; line-height: 1; color: transparent; white-space: pre-wrap;\">{}</p>\n",
                (b.rect[2] - b.rect[0]).max(1.0),
                b.size.max(1.0),
                esc(b.text.trim())
            ));
        }
        body.push_str("</div>\n");
        let head = format!("<meta name=\"viewport\" content=\"width={w}, height={h}\"/>\n");
        let name = format!("page-{}.xhtml", i + 1);
        zip.add(
            &format!("OEBPS/{name}"),
            xhtml(
                meta.title,
                meta.language,
                &head,
                &format!("<span epub:type=\"pagebreak\" role=\"doc-pagebreak\" id=\"page-{n}\" aria-label=\"{n}\"></span>\n{body}", n = i + 1),
            )
            .as_bytes(),
            true,
        );
        zip.add(&format!("OEBPS/{picture}"), &p.picture, false);
        manifest.push((format!("page-{}", i + 1), name, "application/xhtml+xml", None));
        manifest.push((format!("picture-{}", i + 1), picture, media_type(p.ext), None));
        spine.push(format!("page-{}", i + 1));
    }
    let target = |p: usize| format!("page-{}.xhtml", p.min(pages.len().saturating_sub(1)) + 1);
    let entries: Vec<TocEntry> = if toc.is_empty() {
        (0..pages.len()).map(|p| TocEntry { title: format!("{} {}", meta.title, p + 1), level: 1, page: p }).collect()
    } else {
        toc.to_vec()
    };
    let page_list: Vec<(String, String)> = (0..pages.len()).map(|p| ((p + 1).to_string(), target(p))).collect();
    zip.add("OEBPS/nav.xhtml", nav(meta, &entries, &page_list, &|_, e| target(e.page)).as_bytes(), true);
    zip.add("OEBPS/toc.ncx", ncx(meta, &entries, &|_, e| target(e.page)).as_bytes(), true);
    zip.add("OEBPS/content.opf", opf(meta, &manifest, &spine, true).as_bytes(), true);
    zip.finish()
}

/// The navigation document: the table of contents (nested by level) and the PDF's page list.
fn nav(meta: &Meta, entries: &[TocEntry], page_list: &[(String, String)], target: &dyn Fn(usize, &TocEntry) -> String) -> String {
    let mut toc = String::from("<nav epub:type=\"toc\" id=\"toc\">\n<h1>");
    toc.push_str(&esc(meta.title));
    toc.push_str("</h1>\n<ol>\n");
    // Nesting: each entry deeper than the one before opens a list inside it (one level at a time).
    let mut depth = 1usize;
    for (i, e) in entries.iter().enumerate() {
        // The first entry is the top whatever its level; later ones nest at most one level deeper.
        let lvl = if i == 0 { 1 } else { usize::from(e.level.max(1)).min(depth + 1) };
        if i > 0 {
            if lvl > depth {
                toc.push_str("\n<ol>\n");
            } else {
                toc.push_str("</li>\n");
                for _ in lvl..depth {
                    toc.push_str("</ol>\n</li>\n");
                }
            }
        }
        depth = lvl;
        toc.push_str(&format!("<li><a href=\"{}\">{}</a>", esc(&target(i, e)), esc(&e.title)));
    }
    if !entries.is_empty() {
        toc.push_str("</li>\n");
        for _ in 1..depth {
            toc.push_str("</ol>\n</li>\n");
        }
    }
    toc.push_str("</ol>\n</nav>\n");
    let mut list = String::from("<nav epub:type=\"page-list\" id=\"page-list\" hidden=\"\">\n<ol>\n");
    for (label, href) in page_list {
        list.push_str(&format!("<li><a href=\"{}\">{}</a></li>\n", esc(href), esc(label)));
    }
    list.push_str("</ol>\n</nav>\n");
    xhtml(meta.title, meta.language, "", &format!("{toc}{list}"))
}

/// `toc.ncx` for EPUB 2 reading systems: the same entries, flat.
fn ncx(meta: &Meta, entries: &[TocEntry], target: &dyn Fn(usize, &TocEntry) -> String) -> String {
    let mut points = String::new();
    for (i, e) in entries.iter().enumerate() {
        points.push_str(&format!(
            "<navPoint id=\"np-{n}\" playOrder=\"{n}\"><navLabel><text>{}</text></navLabel><content src=\"{}\"/></navPoint>\n",
            esc(&e.title),
            esc(&target(i, e)),
            n = i + 1
        ));
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<ncx xmlns=\"http://www.daisy.org/z3986/2005/ncx/\" version=\"2005-1\">\n<head>\n<meta name=\"dtb:uid\" content=\"{}\"/>\n</head>\n<docTitle><text>{}</text></docTitle>\n<navMap>\n{points}</navMap>\n</ncx>\n",
        esc(meta.identifier),
        esc(meta.title)
    )
}

/// The package document.
fn opf(meta: &Meta, manifest: &[(String, String, &str, Option<&str>)], spine: &[String], fixed: bool) -> String {
    let mut m = String::new();
    for (id, href, media, props) in manifest {
        let props = props.map_or(String::new(), |p| format!(" properties=\"{p}\""));
        m.push_str(&format!("<item id=\"{}\" href=\"{}\" media-type=\"{media}\"{props}/>\n", esc(id), esc(href)));
    }
    let s: String = spine.iter().map(|id| format!("<itemref idref=\"{}\"/>\n", esc(id))).collect();
    let author = meta.author.filter(|a| !a.trim().is_empty()).map_or(String::new(), |a| format!("<dc:creator>{}</dc:creator>\n", esc(a.trim())));
    let layout =
        if fixed { "<meta property=\"rendition:layout\">pre-paginated</meta>\n<meta property=\"rendition:spread\">none</meta>\n" } else { "" };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" unique-identifier=\"bookid\" xml:lang=\"{lang}\">\n<metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n<dc:identifier id=\"bookid\">{}</dc:identifier>\n<dc:title>{}</dc:title>\n<dc:language>{lang}</dc:language>\n{author}<meta property=\"dcterms:modified\">{}</meta>\n{layout}</metadata>\n<manifest>\n{m}</manifest>\n<spine toc=\"ncx\">\n{s}</spine>\n</package>\n",
        esc(meta.identifier),
        esc(meta.title),
        esc(meta.modified),
        lang = esc(meta.language)
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::{Block, Image};
    use std::collections::{HashMap, HashSet};
    use std::io::Read;

    /// XHTML content documents carry `<!DOCTYPE html>`.
    pub(super) fn parse(text: &str) -> Result<roxmltree::Document<'_>, roxmltree::Error> {
        roxmltree::Document::parse_with_options(text, roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() })
    }

    /// The archive's entries in order: (name, stored?, data). A reader for the tests only.
    pub(super) fn entries(zip: &[u8]) -> Vec<(String, bool, Vec<u8>)> {
        let eocd = zip.len() - 22;
        assert_eq!(&zip[eocd..eocd + 4], b"PK\x05\x06");
        let count = u16::from_le_bytes([zip[eocd + 10], zip[eocd + 11]]) as usize;
        let mut at = u32::from_le_bytes(zip[eocd + 16..eocd + 20].try_into().unwrap()) as usize;
        let mut out = Vec::new();
        for _ in 0..count {
            assert_eq!(&zip[at..at + 4], b"PK\x01\x02");
            let method = u16::from_le_bytes([zip[at + 10], zip[at + 11]]);
            let csize = u32::from_le_bytes(zip[at + 20..at + 24].try_into().unwrap()) as usize;
            let nlen = u16::from_le_bytes([zip[at + 28], zip[at + 29]]) as usize;
            let elen = u16::from_le_bytes([zip[at + 30], zip[at + 31]]) as usize;
            let clen = u16::from_le_bytes([zip[at + 32], zip[at + 33]]) as usize;
            let local = u32::from_le_bytes(zip[at + 42..at + 46].try_into().unwrap()) as usize;
            let name = String::from_utf8(zip[at + 46..at + 46 + nlen].to_vec()).unwrap();
            let lnlen = u16::from_le_bytes([zip[local + 26], zip[local + 27]]) as usize;
            let lelen = u16::from_le_bytes([zip[local + 28], zip[local + 29]]) as usize;
            let start = local + 30 + lnlen + lelen;
            let raw = &zip[start..start + csize];
            let data = if method == 0 {
                raw.to_vec()
            } else {
                let mut d = Vec::new();
                flate2::read::DeflateDecoder::new(raw).read_to_end(&mut d).unwrap();
                d
            };
            out.push((name, method == 0 && lelen == 0, data));
            at += 46 + nlen + elen + clen;
        }
        out
    }

    /// epubcheck's structural rules, as assertions.
    fn check(book: &[u8]) -> HashMap<String, Vec<u8>> {
        let all = entries(book);
        // The mimetype file comes first, stored, with no extra field and exactly this content.
        assert_eq!(all[0].0, "mimetype");
        assert!(all[0].1, "mimetype is stored without an extra field");
        assert_eq!(all[0].2, b"application/epub+zip");
        assert_eq!(&book[30..38], b"mimetype", "the local header names it at offset 30");
        let files: HashMap<String, Vec<u8>> = all.into_iter().map(|(n, _, d)| (n, d)).collect();
        let container = String::from_utf8(files["META-INF/container.xml"].clone()).unwrap();
        let c = parse(&container).unwrap();
        let rootfile = c.descendants().find(|n| n.has_tag_name("rootfile")).unwrap().attribute("full-path").unwrap().to_string();
        let opf = String::from_utf8(files[&rootfile].clone()).unwrap();
        let o = parse(&opf).unwrap();
        let base = rootfile.rsplit_once('/').map_or(String::new(), |(d, _)| format!("{d}/"));
        let mut ids = HashSet::new();
        let mut manifested = HashSet::new();
        let mut nav = None;
        for item in o.descendants().filter(|n| n.has_tag_name("item")) {
            let id = item.attribute("id").unwrap();
            assert!(ids.insert(id.to_string()), "duplicate id {id}");
            let path = format!("{base}{}", item.attribute("href").unwrap());
            assert!(files.contains_key(&path), "manifest item {path} is in the archive");
            let media = item.attribute("media-type").unwrap();
            if media == "application/xhtml+xml" || media == "application/x-dtbncx+xml" {
                parse(std::str::from_utf8(&files[&path]).unwrap()).unwrap_or_else(|e| panic!("{path} is well-formed XML: {e}"));
            }
            if item.attribute("properties") == Some("nav") {
                nav = Some(path.clone());
            }
            manifested.insert(path);
        }
        for name in files.keys() {
            if name != "mimetype" && !name.starts_with("META-INF/") && *name != rootfile {
                assert!(manifested.contains(name), "{name} is in the manifest");
            }
        }
        for r in o.descendants().filter(|n| n.has_tag_name("itemref")) {
            assert!(ids.contains(r.attribute("idref").unwrap()), "spine idref resolves");
        }
        let uid = o.root_element().attribute("unique-identifier").unwrap();
        assert!(o.descendants().any(|n| n.has_tag_name("identifier") && n.attribute("id") == Some(uid)));
        assert!(o.descendants().any(|n| n.attribute("property") == Some("dcterms:modified")));
        let nav = String::from_utf8(files[&nav.expect("a navigation document")].clone()).unwrap();
        let n = parse(&nav).unwrap();
        assert!(
            n.descendants().any(|e| e.has_tag_name("nav") && e.attributes().any(|a| a.name() == "type" && a.value() == "toc")),
            "nav has a toc nav"
        );
        files
    }

    pub(super) fn meta() -> Meta<'static> {
        Meta {
            title: "Report & notes",
            author: Some("Ada"),
            language: "en",
            identifier: "urn:uuid:00000000-0000-5000-8000-000000000000",
            modified: "2026-10-16T12:00:00Z",
        }
    }

    pub(super) fn block(text: &str, size: f64, top: f64) -> Block {
        Block {
            text: text.into(),
            rect: [72.0, top - size, 400.0, top],
            size,
            bold: false,
            italic: false,
            color: [0.0; 3],
            rotation: 0.0,
            space_around: [false; 2],
        }
    }

    pub(super) fn page(blocks: Vec<Block>) -> Page {
        Page { width: 612.0, height: 792.0, blocks, images: Vec::new(), background: None }
    }

    #[test]
    fn a_reflowable_book_passes_the_structural_rules_and_splits_at_headings() {
        let png = b"\x89PNG\r\n\x1a\nfake".to_vec();
        let mut p1 = page(vec![block("Introduction", 24.0, 700.0), block("Body text of the first page.", 11.0, 650.0)]);
        p1.images.push(Image { ext: "png", bytes: png.into(), rect: [72.0, 300.0, 300.0, 500.0], size: [228.0, 200.0], rotation: 0.0 });
        let p2 = page(vec![block("More body < & > text.", 11.0, 700.0)]);
        let p3 = page(vec![block("Second chapter", 24.0, 700.0), block("Its body.", 11.0, 650.0)]);
        let book = epub(&[p1, p2, p3], &meta(), &[], &Tags::default());
        let files = check(&book);
        assert!(files.contains_key("OEBPS/chapter-1.xhtml") && files.contains_key("OEBPS/chapter-2.xhtml"), "a chapter per top-level heading");
        let ch1 = String::from_utf8(files["OEBPS/chapter-1.xhtml"].clone()).unwrap();
        assert!(ch1.contains("<h1 id=\"h-1\">Introduction</h1>"));
        assert!(ch1.contains("More body &lt; &amp; &gt; text."));
        assert!(ch1.contains("id=\"page-2\""), "the PDF's page numbers are kept");
        assert!(ch1.contains("<img src=\"images/img-1.png\""));
        let ch2 = String::from_utf8(files["OEBPS/chapter-2.xhtml"].clone()).unwrap();
        assert!(ch2.contains("id=\"page-3\"") && ch2.contains("Second chapter"));
        let nav = String::from_utf8(files["OEBPS/nav.xhtml"].clone()).unwrap();
        assert!(nav.contains("chapter-2.xhtml#h-2\">Second chapter</a>"));
        assert!(nav.contains("page-list"));
        let opf = String::from_utf8(files["OEBPS/content.opf"].clone()).unwrap();
        assert!(opf.contains("<dc:creator>Ada</dc:creator>") && opf.contains("<dc:title>Report &amp; notes</dc:title>"));
    }

    #[test]
    fn tags_give_heading_levels_and_alternate_text() {
        let mut p1 = page(vec![block("Plain looking heading", 11.0, 700.0), block("Big but not a heading", 30.0, 650.0)]);
        p1.images.push(Image {
            ext: "jpg",
            bytes: b"\xff\xd8\xff".as_slice().into(),
            rect: [72.0, 300.0, 300.0, 500.0],
            size: [228.0, 200.0],
            rotation: 0.0,
        });
        let tags = Tags {
            headings: vec![(0, 2, "Plain  looking heading".into())],
            alts: vec![(0, [70.0, 290.0, 310.0, 510.0], "A bar chart of sales".into())],
        };
        let book = epub(&[p1], &meta(), &[], &tags);
        let files = check(&book);
        let ch = String::from_utf8(files["OEBPS/chapter-1.xhtml"].clone()).unwrap();
        assert!(ch.contains("<h2 id=\"h-1\">Plain looking heading</h2>"), "{ch}");
        assert!(ch.contains("<p>Big but not a heading</p>"), "a tagged document's headings come from its tags only");
        assert!(ch.contains("alt=\"A bar chart of sales\""));
    }

    #[test]
    fn bookmarks_nest_in_the_table_of_contents() {
        let pages: Vec<Page> = (0..3).map(|i| page(vec![block(&format!("Page {i}"), 11.0, 700.0)])).collect();
        let toc = vec![
            TocEntry { title: "Part one".into(), level: 1, page: 0 },
            TocEntry { title: "Section".into(), level: 2, page: 1 },
            TocEntry { title: "Part two".into(), level: 1, page: 2 },
        ];
        let files = check(&epub(&pages, &meta(), &toc, &Tags::default()));
        let nav = String::from_utf8(files["OEBPS/nav.xhtml"].clone()).unwrap();
        let n = parse(&nav).unwrap();
        let section = n.descendants().find(|e| e.has_tag_name("a") && e.text() == Some("Section")).unwrap();
        let depth = section.ancestors().filter(|a| a.has_tag_name("ol")).count();
        assert_eq!(depth, 2, "a level-2 bookmark sits inside its parent's list");
    }

    #[test]
    fn a_fixed_layout_book_has_a_page_per_pdf_page_with_text_over_the_picture() {
        let pages: Vec<FixedPage> = (0..2)
            .map(|i| FixedPage {
                width: 612.0,
                height: 792.0,
                ext: "jpg",
                picture: b"\xff\xd8\xff".to_vec(),
                page: page(vec![block(&format!("Searchable {i}"), 12.0, 700.0)]),
            })
            .collect();
        let files = check(&epub_fixed(&pages, &meta(), &[]));
        let opf = String::from_utf8(files["OEBPS/content.opf"].clone()).unwrap();
        assert!(opf.contains("<meta property=\"rendition:layout\">pre-paginated</meta>"));
        let p2 = String::from_utf8(files["OEBPS/page-2.xhtml"].clone()).unwrap();
        assert!(p2.contains("content=\"width=612, height=792\""));
        assert!(p2.contains("Searchable 1") && p2.contains("color: transparent"));
        assert!(files.contains_key("OEBPS/images/page-2.jpg"));
    }

    #[test]
    fn dates_and_identifiers() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_utc(1_792_108_800), "2026-10-16T00:00:00Z");
        assert_eq!(iso_utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(iso_utc(i64::MAX), "1970-01-01T00:00:00Z");
        let u = urn_uuid(&[0xab; 32]);
        assert_eq!(u, "urn:uuid:abababab-abab-5bab-abab-abababababab");
    }
}

#[cfg(test)]
mod nav_targets;
