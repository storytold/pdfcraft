//! Work regression for the forms a redaction reads besides its walk, when their `/DecodeParms` is
//! given by reference: each is decoded through a view that shares its data and copies no more of
//! its dictionary than its filters and parameters, not through a copy of the whole stream, which
//! the unlinking check, reading a form once for each page that draws it, would make again for
//! every page. A test binary of its own, so that nothing else runs while it is timed.

use std::sync::Arc;
use std::time::{Duration, Instant};

use pdfcraft_annot::{Meta, NewAnnotation, Shape, Style, add_annotation, rect_quad};
use pdfcraft_cos::Document;
use pdfcraft_redact::apply;
use pdfcraft_testkit::{Pdf, stream_obj};

const IMAGE: &str = "/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8";

/// Pages that each draw it.
const PAGES: usize = 256;

/// `PAGES` pages sharing the resources of their `/Pages` node: the first draws an image under the
/// mark, and every page draws a form without resources of its own outside it, whose dictionary
/// holds an unrelated array of a quarter of a million numbers, and whose `/DecodeParms`, empty, is
/// a reference when `by_reference`. The parsed form stays in the document's object cache: one past
/// the cache's budget is parsed again at each read once another object is loaded, whatever its
/// parameters, which this doesn't measure.
fn pages(by_reference: bool) -> Document {
    let kids: Vec<String> = std::iter::once("3 0 R".to_string()).chain((1..PAGES).map(|i| format!("{} 0 R", 8 + i))).collect();
    let parms = if by_reference { "7 0 R" } else { "<< >>" };
    let junk = vec!["0"; 1 << 18].join(" ");
    let content: Vec<u8> = b"200 200 m 250 250 l S".iter().flat_map(|b| format!("{b:02X}").into_bytes()).chain(*b">").collect();
    let mut objs = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!("<< /Type /Pages /Kids [{}] /Count {PAGES} /Resources << /XObject << /Im 5 0 R /Fm 6 0 R >> >> >>", kids.join(" ")).into_bytes(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R >>".to_vec(),
        stream_obj("", b"q 20 0 0 20 50 50 cm /Im Do Q /Fm Do"),
        stream_obj(IMAGE, b"o"),
        stream_obj(
            &format!("/Type /XObject /Subtype /Form /BBox [200 200 250 250] /Filter /ASCIIHexDecode /DecodeParms {parms} /Junk [{junk}]"),
            content,
        ),
        b"<< >>".to_vec(),
        stream_obj("", b"/Fm Do"),
    ];
    objs.extend((1..PAGES).map(|_| b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 8 0 R >>".to_vec()));
    let mut doc = Document::open(Arc::new(Pdf::new().objs(objs).build())).unwrap();
    let shape = Shape::Redact { quads: vec![rect_quad([40.0, 40.0, 80.0, 80.0])], overlay: String::new(), look: Default::default() };
    let style = Style::default_for(&shape);
    add_annotation(&mut doc, &NewAnnotation { page: 0, shape, style, contents: String::new(), author: "Synthetic test".into() }, &Meta::default())
        .unwrap();
    doc
}

/// How long redacting `pages(by_reference)` takes, the least of three runs.
fn redaction(by_reference: bool) -> Duration {
    (0..3)
        .map(|_| {
            let mut doc = pages(by_reference);
            let start = Instant::now();
            let report = apply(&mut doc, None).unwrap();
            let took = start.elapsed();
            assert_eq!(report.images_removed, 1, "the marked page's image goes");
            took
        })
        .min()
        .unwrap_or_default()
}

/// The unlinking check reads the form once for each of the pages that draw it. Through a copy of
/// the stream, each read copied the array again: with its parameters by reference, the redaction
/// took about 28 times as long as with them direct (560 ms against 20, measured in the test
/// profile). Through a view, the two take about as long.
#[test]
fn a_form_read_for_each_page_is_not_copied_for_each_page() {
    let (direct, by_reference) = (redaction(false), redaction(true));
    assert!(by_reference < direct * 3, "by reference {by_reference:?}, direct {direct:?}");
}
