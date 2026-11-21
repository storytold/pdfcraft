//! An image Apply rewrites keeps no XMP packet of the image as it was (#7981).

use super::*;

/// A thumbnail payload no other part of the file holds, so finding it in the saved bytes means
/// the packet survived.
const THUMB: &str = "UElDVFVSRU9GVEhFSU1BR0VBU0lUV0FT";

fn image_with_xmp() -> Document {
    let packet = format!(
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
         <rdf:Description xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\" xmlns:xmpGImg=\"http://ns.adobe.com/xap/1.0/g/img/\">\
         <xmp:Thumbnails><rdf:Alt><rdf:li><xmpGImg:image>{THUMB}</xmpGImg:image></rdf:li></rdf:Alt></xmp:Thumbnails>\
         </rdf:Description></rdf:RDF></x:xmpmeta>"
    );
    one_page(
        b"q 200 0 0 100 0 0 cm /Im Do Q",
        "/XObject << /Im 7 0 R >>",
        vec![
            stream("/Type /XObject /Subtype /Image /Width 2 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Metadata 8 0 R", [123, 234]),
            stream("/Type /Metadata /Subtype /XML", packet.as_bytes()),
        ],
    )
}

/// A mark over half of an image clears the covered pixel in a copy. The copy carries none of the
/// original's `/Metadata`, so its thumbnail, a picture of the pixel the mark covers, is not in the
/// saved file. The image's other half is still drawn.
#[test]
fn a_cleared_image_keeps_no_thumbnail_of_itself() {
    let mut doc = image_with_xmp();
    let before = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(before.windows(THUMB.len()).any(|w| w == THUMB.as_bytes()), "the fixture's packet is written before Apply");

    mark(&mut doc, 0, &[[10.0, 10.0, 20.0, 90.0]], "");
    let report = apply(&mut doc, None).unwrap();
    assert_eq!((report.images_cleared, report.images_removed), (1, 0), "{report:?}");

    let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(!bytes.windows(THUMB.len()).any(|w| w == THUMB.as_bytes()), "the image's thumbnail survived Apply");
    let saved = Document::open(Arc::new(bytes)).unwrap();
    assert_eq!(named_image(&saved, 0, b"PCRedacted1"), Some(vec![0, 234]));
}
