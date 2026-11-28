//! #7921: an XMP thumbnail (`xmp:Thumbnails`, an `xmpGImg:image` in base 64) pictures the page
//! as it was before the marks were applied. No text search can find what it shows, so applying
//! removes the catalog's and pages' packets that carry one, keeping the catalog's PDF/A
//! identification as for a packet that held the removed text.

use super::*;
use crate::{Claim, Identification};

const HEAD: &str = r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?><x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">"#;
const TAIL: &str = r#"</rdf:RDF></x:xmpmeta><?xpacket end="w"?>"#;
/// Base 64 of the bytes 0 to 31: synthetic picture data, not an image of anything.
const PICTURE: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";

/// A packet whose only text is a neutral title, with a thumbnail under the usual prefixes.
fn with_thumbnail(extra: &str) -> String {
    format!(
        r#"{HEAD}{extra}<rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:xmpGImg="http://ns.adobe.com/xap/1.0/g/img/"><dc:title>Quarterly sheet</dc:title><xmp:Thumbnails><rdf:Alt><rdf:li rdf:parseType="Resource"><xmpGImg:format>JPEG</xmpGImg:format><xmpGImg:width>8</xmpGImg:width><xmpGImg:height>4</xmpGImg:height><xmpGImg:image>{PICTURE}</xmpGImg:image></rdf:li></rdf:Alt></xmp:Thumbnails></rdf:Description>{TAIL}"#
    )
}

/// A packet with no thumbnail and nothing the page shows.
fn plain() -> String {
    format!(
        r#"{HEAD}<rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Quarterly sheet</dc:title></rdf:Description>{TAIL}"#
    )
}

/// One page showing "Total MARKER-SECRET due", with catalog XMP `catalog` (object 7) and page
/// XMP `page` (object 8).
fn document(catalog: &str, page: &str) -> Document {
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /Metadata 7 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Metadata 8 0 R >>".to_vec(),
        stream("", b"BT /F1 10 Tf 10 100 Td (Total MARKER-SECRET due) Tj ET"),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        stream("/Type /Metadata /Subtype /XML", catalog.as_bytes()),
        stream("/Type /Metadata /Subtype /XML", page.as_bytes()),
    ];
    Document::open(Arc::new(Pdf::new().objs(objs).build())).unwrap()
}

/// The XMP text of `holder`'s `/Metadata`, if it has one.
fn xmp_of(doc: &Document, holder: pdfcraft_cos::ObjRef) -> Option<String> {
    let xmp = doc.get(holder).as_dict().and_then(|d| d.get(b"Metadata").cloned())?;
    crate::metadata::xmp_text(doc, &xmp)
}

fn has(haystack: &[u8], needle: &str) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle.as_bytes())
}

/// The issue's case: the marker is only in the page content, the catalog packet's thumbnail
/// pictures the page. After applying a mark over the marker and saving with a full rewrite, no
/// packet in the file holds the picture; the catalog's PDF/A identification is written back, and
/// the removal is counted.
#[test]
fn a_catalog_xmp_thumbnail_does_not_survive_apply() {
    let pdfa = r#"<rdf:Description rdf:about="" xmlns:pdfaid="http://www.aiim.org/pdfa/ns/id/" pdfaid:part="2" pdfaid:conformance="B"/>"#;
    let mut doc = document(&with_thumbnail(pdfa), &plain());
    mark(&mut doc, 0, &[[35.0, 95.0, 105.0, 110.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert!(r.glyphs > 0, "{r:?}");
    assert_eq!(r.metadata, 1, "the catalog packet is counted: {r:?}");
    assert_eq!(r.identification, Identification { pdfa: Claim::Kept, pdfua: Claim::None }, "{r:?}");

    let saved = write_full(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap();
    assert!(!has(&saved, PICTURE) && !has(&saved, "xmpGImg"), "the thumbnail is in the saved file");
    let reopened = Document::open(Arc::new(saved)).unwrap();
    let catalog = xmp_of(&reopened, reopened.root().unwrap()).expect("the identification is written back");
    assert!(!catalog.contains("xmpGImg") && !catalog.contains("Thumbnails") && !catalog.contains("Quarterly"), "{catalog}");
    assert!(catalog.contains("pdfaid"), "{catalog}");
    // The page's packet carries no thumbnail and nothing the page showed: it stays.
    let page = pdfcraft_model::pages(&reopened)[0].obj;
    assert!(xmp_of(&reopened, page).is_some_and(|t| t.contains("Quarterly")), "the plain page packet stays");
}

/// A page's packet with a thumbnail goes too, found by its namespace under a prefix of its own,
/// and so it does when the mark removed no text at all (an empty area of the page).
#[test]
fn a_page_xmp_thumbnail_goes_even_when_no_text_was_removed() {
    let renamed = with_thumbnail("").replace("xmpGImg", "gi").replace("xmp:Thumbnails", "xmp:Previews");
    assert!(!renamed.contains("Thumbnails"), "only the namespace can tell");
    let mut doc = document(&plain(), &renamed);
    mark(&mut doc, 0, &[[10.0, 200.0, 100.0, 250.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.glyphs, 0, "{r:?}");
    assert_eq!(r.metadata, 1, "{r:?}");
    assert_eq!(r.identification, Identification::default(), "{r:?}");
    let page = pdfcraft_model::pages(&doc)[0].obj;
    assert_eq!(xmp_of(&doc, page), None, "the page packet goes");
    assert!(xmp_of(&doc, doc.root().unwrap()).is_some_and(|t| t.contains("Quarterly")), "the plain catalog packet stays");
}
