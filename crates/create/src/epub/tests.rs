//! Books built here, converted, and the PDF checked object by object.

use super::*;
use crate::epub::zip::tests::zip;

const CONTAINER: &[u8] = br#"<?xml version="1.0"?><container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;

fn opf(manifest: &str, spine: &str, extra_meta: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">urn:uuid:12345678-1234-5678-1234-567812345678</dc:identifier>
<dc:title>A Small Book</dc:title><dc:creator>Ana Writer</dc:creator><dc:language>es</dc:language>{extra_meta}</metadata>
<manifest>{manifest}</manifest><spine>{spine}</spine></package>"#
    )
}

fn xhtml(head: &str, body: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE html><html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><head><title>t</title>{head}</head><body>{body}</body></html>"#
    )
}

fn png(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc.write_header().unwrap();
    wr.write_image_data(&vec![200u8; (w * h * 3) as usize]).unwrap();
    wr.finish().unwrap();
    out
}

fn book(files: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut entries: Vec<(&str, &[u8], bool)> = vec![("mimetype", b"application/epub+zip", false), ("META-INF/container.xml", CONTAINER, true)];
    entries.extend(files.iter().map(|(n, b)| (*n, b.as_slice(), true)));
    zip(&entries)
}

fn catalog(doc: &Document) -> Dict {
    doc.root().and_then(|r| doc.get(r).as_dict().cloned()).unwrap()
}

fn pages(doc: &Document) -> Vec<Dict> {
    pdfcraft_model::pages(doc).iter().map(|p| doc.get(p.obj).as_dict().cloned().unwrap()).collect()
}

fn reflow_book() -> Vec<u8> {
    let manifest = r#"<item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
        <item id="c1" href="text/c1.xhtml" media-type="application/xhtml+xml"/><item id="c2" href="text/c2.xhtml" media-type="application/xhtml+xml"/>
        <item id="css" href="css/s.css" media-type="text/css"/><item id="img" href="img/p.png" media-type="image/png"/>"#;
    let nav = xhtml(
        "",
        r#"<nav epub:type="toc"><ol><li><a href="text/c1.xhtml">One</a><ol><li><a href="text/c2.xhtml#later">Later</a></li></ol></li></ol></nav>"#,
    );
    let para = "<p>Words that fill a page, with enough of them to wrap onto several lines and pages of the book.</p>".repeat(40);
    let c1 = xhtml(
        r#"<link rel="stylesheet" href="../css/s.css"/>"#,
        &format!(
            r#"<h1>Chapter One</h1><p class="lead">See <a href="c2.xhtml#later">later</a> or <a href="https://example.com/">the site</a>.</p><img src="../img/p.png" alt="grey"/>{para}"#
        ),
    );
    let c2 = xhtml("", r#"<h2 id="later">Later</h2><p>The end — fin.</p>"#);
    book(&[
        ("OEBPS/content.opf", opf(manifest, r#"<itemref idref="c1"/><itemref idref="c2"/>"#, "").into_bytes()),
        ("OEBPS/nav.xhtml", nav.into_bytes()),
        ("OEBPS/text/c1.xhtml", c1.into_bytes()),
        ("OEBPS/text/c2.xhtml", c2.into_bytes()),
        ("OEBPS/css/s.css", b"h1 { text-align: center } .lead { font-style: italic }".to_vec()),
        ("OEBPS/img/p.png", png(40, 20)),
    ])
}

#[test]
fn a_reflowable_book_becomes_pages_with_its_contents_links_and_metadata() {
    let bytes = reflow_book();
    assert!(is_epub("book.epub", &bytes) && is_epub("renamed.zip", &bytes));
    let out = from_epub(&bytes, LETTER, "book").unwrap();
    let doc = &out.document;
    let ps = pages(doc);
    assert!(ps.len() >= 3, "{} pages", ps.len());
    assert_eq!(ps[0].get(b"MediaBox").and_then(|m| m.as_array().map(|a| a.len())), Some(4));
    // The contents as bookmarks: One, with Later inside it.
    let cat = catalog(doc);
    let outlines = doc.dict(cat.get(b"Outlines").unwrap()).unwrap();
    let first = doc.dict(outlines.get(b"First").unwrap()).unwrap();
    assert_eq!(first.get(b"Title").and_then(|t| t.as_string().map(|s| s.to_text())).as_deref(), Some("One"));
    let child = doc.dict(first.get(b"First").unwrap()).unwrap();
    assert_eq!(child.get(b"Title").and_then(|t| t.as_string().map(|s| s.to_text())).as_deref(), Some("Later"));
    // The internal link goes to a page; the external one is a URI action.
    let annots: Vec<Dict> =
        ps[0].get(b"Annots").and_then(|a| doc.resolve(a).as_array().cloned()).unwrap().iter().map(|a| doc.dict(a).unwrap()).collect();
    assert!(annots.iter().any(|a| a.get(b"Dest").is_some()));
    assert!(annots.iter().any(|a| a.get(b"A").and_then(|x| doc.dict(x)).and_then(|x| x.get(b"URI").cloned()).is_some()));
    // The picture is on the first page; metadata and language came along.
    let res = doc.dict(ps[0].get(b"Resources").unwrap()).unwrap();
    assert!(res.get(b"XObject").is_some());
    let info = doc.dict(doc.trailer().get(b"Info").unwrap()).unwrap();
    assert_eq!(info.get(b"Title").and_then(|t| t.as_string().map(|s| s.to_text())).as_deref(), Some("A Small Book"));
    assert_eq!(info.get(b"Author").and_then(|t| t.as_string().map(|s| s.to_text())).as_deref(), Some("Ana Writer"));
    assert_eq!(cat.get(b"Lang").and_then(|t| t.as_string().map(|s| s.to_text())).as_deref(), Some("es"));
    assert!(out.notes[0].contains("US Letter"), "{:?}", out.notes);
    // It saves and opens again.
    let saved = pdfcraft_cos::write_full(doc, &pdfcraft_cos::SaveOptions::default()).unwrap();
    assert_eq!(pdfcraft_model::pages(&Document::open(std::sync::Arc::new(saved)).unwrap()).len(), ps.len());
}

#[test]
fn a_fixed_layout_book_is_a_page_per_document_at_its_viewport() {
    let manifest = r#"<item id="p1" href="p1.xhtml" media-type="application/xhtml+xml"/><item id="i" href="p1.png" media-type="image/png"/>"#;
    let page = xhtml(r#"<meta name="viewport" content="width=600, height=800"/>"#, r#"<div><img src="p1.png" alt=""/></div>"#);
    let bytes = book(&[
        ("OEBPS/content.opf", opf(manifest, r#"<itemref idref="p1"/>"#, r#"<meta property="rendition:layout">pre-paginated</meta>"#).into_bytes()),
        ("OEBPS/p1.xhtml", page.into_bytes()),
        ("OEBPS/p1.png", png(60, 80)),
    ]);
    let out = from_epub(&bytes, A4, "comic").unwrap();
    let ps = pages(&out.document);
    assert_eq!(ps.len(), 1);
    let mb: Vec<f64> = ps[0].get(b"MediaBox").unwrap().as_array().unwrap().iter().map(|o| o.as_f64().unwrap()).collect();
    assert_eq!(mb, [0.0, 0.0, 450.0, 600.0]);
    assert!(!out.notes.iter().any(|n| n.contains("paper")), "no reflow, so no paper note: {:?}", out.notes);
}

#[test]
fn a_protected_book_is_refused_with_a_plain_reason() {
    let enc = br#"<encryption xmlns="urn:oasis:names:tc:opendocument:xmlns:container" xmlns:enc="http://www.w3.org/2001/04/xmlenc#"><enc:EncryptedData><enc:EncryptionMethod Algorithm="http://www.w3.org/2001/04/xmlenc#aes128-cbc"/><enc:CipherData><enc:CipherReference URI="OEBPS/text/c1.xhtml"/></enc:CipherData></enc:EncryptedData></encryption>"#;
    let mut files = vec![("META-INF/encryption.xml", enc.to_vec())];
    files.push(("OEBPS/content.opf", opf("", "", "").into_bytes()));
    let err = from_epub(&book(&files), LETTER, "x").unwrap_err();
    assert_eq!(err, EpubError::Protected);
    let message = err.to_string();
    assert!(message.contains("DRM") && !message.to_lowercase().contains("remove") && !message.to_lowercase().contains("calibre"), "{message}");
}

#[test]
fn an_obfuscated_book_font_is_undone_and_embedded() {
    let mut font = pdfcraft_fonts::INTER_REGULAR.to_vec();
    package::deobfuscate(&mut font, package::Obfuscation::Idpf, "urn:uuid:12345678-1234-5678-1234-567812345678");
    let enc = br#"<encryption xmlns="urn:oasis:names:tc:opendocument:xmlns:container" xmlns:enc="http://www.w3.org/2001/04/xmlenc#"><enc:EncryptedData><enc:EncryptionMethod Algorithm="http://www.idpf.org/2008/embedding"/><enc:CipherData><enc:CipherReference URI="OEBPS/f.ttf"/></enc:CipherData></enc:EncryptedData></encryption>"#;
    let manifest = r#"<item id="c" href="c.xhtml" media-type="application/xhtml+xml"/><item id="f" href="f.ttf" media-type="font/ttf"/>"#;
    let c = xhtml(
        "<style>@font-face { font-family: 'Book Sans'; src: url(f.ttf) } body { font-family: 'Book Sans', serif }</style>",
        "<p>Hello in the book's face</p>",
    );
    let bytes = book(&[
        ("META-INF/encryption.xml", enc.to_vec()),
        ("OEBPS/content.opf", opf(manifest, r#"<itemref idref="c"/>"#, "").into_bytes()),
        ("OEBPS/c.xhtml", c.into_bytes()),
        ("OEBPS/f.ttf", font),
    ]);
    let out = from_epub(&bytes, LETTER, "x").unwrap();
    assert!(!out.notes.iter().any(|n| n.contains("can't be embedded")), "{:?}", out.notes);
    let doc = &out.document;
    let res = doc.dict(pages(doc)[0].get(b"Resources").unwrap()).unwrap();
    let fonts = doc.dict(res.get(b"Font").unwrap()).unwrap();
    let names: Vec<Vec<u8>> = fonts.iter().filter_map(|(_, f)| doc.dict(f).and_then(|d| d.name(b"BaseFont").map(<[u8]>::to_vec))).collect();
    assert!(names.iter().any(|n| n == b"BookSans"), "{:?}", names.iter().map(|n| String::from_utf8_lossy(n).into_owned()).collect::<Vec<_>>());
}

#[test]
fn a_chapter_that_is_not_well_formed_keeps_its_words() {
    let manifest = r#"<item id="c" href="c.xhtml" media-type="application/xhtml+xml"/>"#;
    let bytes = book(&[
        ("OEBPS/content.opf", opf(manifest, r#"<itemref idref="c"/>"#, "").into_bytes()),
        ("OEBPS/c.xhtml", b"<html><body><p>Unclosed <b>tags<p>Second &amp; last</body></html>".to_vec()),
    ]);
    let out = from_epub(&bytes, A4, "x").unwrap();
    assert!(out.notes.iter().any(|n| n.contains("not well-formed")), "{:?}", out.notes);
    assert!(out.notes[0].contains("A4"));
    assert_eq!(pages(&out.document).len(), 1);
}

#[test]
fn the_region_decides_the_paper() {
    assert_eq!(paper_for_region("PR"), LETTER);
    assert_eq!(paper_for_region("us"), LETTER);
    assert_eq!(paper_for_region("ES"), A4);
    assert_eq!(region_of("es_PR.UTF-8").as_deref(), Some("PR"));
    assert_eq!(region_of("zh-Hant-TW").as_deref(), Some("TW"));
    assert_eq!(region_of("C"), None);
}
