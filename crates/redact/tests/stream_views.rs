//! Work regressions for the forms a redaction's unlinking check reads, when their `/DecodeParms`
//! is given by reference: each is decoded through a view that shares its data and copies no more
//! of its dictionary than its filters and parameters, not through a copy of the whole stream, and
//! once in the check however many pages draw it. A test binary of its own, so that none of the
//! crate's other tests shares its process. Each test compares two timings of its own, the least of
//! three runs each, with the parameters by reference and direct, not a fixed time.

use std::sync::Arc;
use std::time::{Duration, Instant};

use pdfcraft_annot::{Meta, NewAnnotation, Shape, Style, add_annotation, rect_quad};
use pdfcraft_cos::Document;
use pdfcraft_redact::apply;
use pdfcraft_testkit::{Pdf, stream_obj};

const IMAGE: &str = "/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8";

/// `pages` pages sharing the resources of their `/Pages` node: the first draws an image under the
/// mark, and every page draws a form without resources of its own outside it, whose dictionary
/// holds an unrelated array of `numbers` numbers, and whose `/DecodeParms`, empty, is a reference
/// when `by_reference`.
fn document(pages: usize, numbers: usize, by_reference: bool) -> Document {
    let kids: Vec<String> = std::iter::once("3 0 R".to_string()).chain((1..pages).map(|i| format!("{} 0 R", 8 + i))).collect();
    let parms = if by_reference { "7 0 R" } else { "<< >>" };
    let junk = vec!["0"; numbers].join(" ");
    let content: Vec<u8> = b"200 200 m 250 250 l S".iter().flat_map(|b| format!("{b:02X}").into_bytes()).chain(*b">").collect();
    let mut objs = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!("<< /Type /Pages /Kids [{}] /Count {pages} /Resources << /XObject << /Im 5 0 R /Fm 6 0 R >> >> >>", kids.join(" ")).into_bytes(),
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
    objs.extend((1..pages).map(|_| b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 8 0 R >>".to_vec()));
    let mut doc = Document::open(Arc::new(Pdf::new().objs(objs).build())).unwrap();
    let shape = Shape::Redact { quads: vec![rect_quad([40.0, 40.0, 80.0, 80.0])], overlay: String::new(), look: Default::default() };
    let style = Style::default_for(&shape);
    add_annotation(&mut doc, &NewAnnotation { page: 0, shape, style, contents: String::new(), author: "Synthetic test".into() }, &Meta::default())
        .unwrap();
    doc
}

/// How long redacting `document(pages, numbers, by_reference)` takes, the least of three runs.
fn redaction(pages: usize, numbers: usize, by_reference: bool) -> Duration {
    (0..3)
        .map(|_| {
            let mut doc = document(pages, numbers, by_reference);
            let start = Instant::now();
            let report = apply(&mut doc, None).unwrap();
            let took = start.elapsed();
            assert_eq!(report.images_removed, 1, "the marked page's image goes");
            took
        })
        .min()
        .unwrap_or_default()
}

/// Written when the unlinking check read the form for each of the 256 pages that draw it: through a
/// copy of the stream, each read copied the array, a quarter of a million numbers, again, and with
/// its parameters by reference the redaction took about 550 ms against 20 ms with them direct (test
/// profile). The check now looks each form up once (below), so this times one read of it, by
/// reference against direct; `the_reads_besides_the_walk_decode_through_a_view` holds the reads to
/// the view.
#[test]
fn a_form_read_for_each_page_is_not_copied_for_each_page() {
    let (direct, by_reference) = (redaction(256, 1 << 18, false), redaction(256, 1 << 18, true));
    assert!(by_reference < direct * 3, "by reference {by_reference:?}, direct {direct:?}");
}

/// A form whose dictionary is past the document's object cache (32 MB parsed: an array of a
/// million numbers is about 56 MB) is parsed again whenever it is read after another object was
/// loaded, and resolving its parameters loads one. Read for each of 48 pages, by reference it took
/// 4.1 s against 0.48 s direct (test profile). The check now reads each form once for all of them.
#[test]
fn a_form_past_the_object_cache_is_read_once_for_all_pages() {
    let (direct, by_reference) = (redaction(48, 1 << 20, false), redaction(48, 1 << 20, true));
    assert!(by_reference < direct * 3, "by reference {by_reference:?}, direct {direct:?}");
}
