//! An `@font-face` with several sources uses the first that embeds; a note about a source that
//! can't be embedded is made only when none of them can.

use super::*;
use crate::epub::zip::tests::zip;

const CONTAINER: &[u8] = br#"<?xml version="1.0"?><container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;

fn convert(src: &str, files: &[(&str, Vec<u8>)]) -> Vec<String> {
    let opf = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">urn:uuid:12345678-1234-5678-1234-567812345678</dc:identifier>
<dc:title>T</dc:title><dc:language>en</dc:language></metadata>
<manifest><item id="c" href="c.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c"/></spine></package>"#;
    let page = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><html xmlns="http://www.w3.org/1999/xhtml"><head><title>t</title><style>@font-face {{ font-family: 'Book Sans'; src: {src} }} body {{ font-family: 'Book Sans', serif }}</style></head><body><p>Hello</p></body></html>"#
    );
    let mut entries: Vec<(&str, &[u8], bool)> =
        vec![("mimetype", b"application/epub+zip", false), ("META-INF/container.xml", CONTAINER, true), ("OEBPS/content.opf", opf.as_bytes(), true)];
    entries.push(("OEBPS/c.xhtml", page.as_bytes(), true));
    entries.extend(files.iter().map(|(n, b)| (*n, b.as_slice(), true)));
    from_epub(&zip(&entries), LETTER, "x").unwrap().notes
}

fn embed_notes(notes: &[String]) -> Vec<&String> {
    notes.iter().filter(|n| n.contains("can't be embedded")).collect()
}

#[test]
fn a_missing_first_source_is_not_mentioned_when_the_second_embeds() {
    let font = pdfcraft_fonts::INTER_REGULAR.to_vec();
    let notes = convert("url(missing.ttf), url(f1.ttf), url(f2.ttf)", &[("OEBPS/f1.ttf", font.clone()), ("OEBPS/f2.ttf", font)]);
    assert!(embed_notes(&notes).is_empty(), "{notes:?}");
}

#[test]
fn every_source_is_mentioned_when_none_embeds() {
    let notes = convert("url(a.ttf), url(b.ttf)", &[]);
    let said = embed_notes(&notes);
    assert_eq!(said.len(), 2, "{notes:?}");
    assert!(said[0].starts_with("OEBPS/a.ttf") && said[1].starts_with("OEBPS/b.ttf"), "{said:?}");
}
