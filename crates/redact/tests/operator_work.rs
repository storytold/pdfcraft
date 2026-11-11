//! Work regressions for what a redaction looks up at operators a stream can hold thousands of: the
//! tags cleanup's named property lists at each `BDC` are not read whole each time. A test binary of
//! its own, so that none of the crate's other tests shares its process, and its tests run one at a
//! time. Each test compares two timings of its own, the least of three runs each, with many
//! entries or operators and with one, not a fixed time. The bound leaves room for slower machines:
//! each regression took seconds against milliseconds.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use pdfcraft_annot::{Meta, NewAnnotation, Shape, Style, add_annotation, rect_quad};
use pdfcraft_cos::Document;
use pdfcraft_redact::apply;
use pdfcraft_testkit::{Pdf, stream_obj};

/// One page with `resources` and `extra` objects (from object 5 on), drawing a word under a mark
/// (so the redaction rewrites the page) and then `ops`.
fn page(resources: &str, ops: &[u8], extra: Vec<Vec<u8>>) -> Document {
    let content = [b"BT /F1 10 Tf 20 260 Td (SECRET) Tj ET ".as_slice(), ops].concat();
    let mut objs = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> {resources} >> >>"
        )
        .into_bytes(),
        stream_obj("", content),
    ];
    objs.extend(extra);
    let mut doc = Document::open(Arc::new(Pdf::new().objs(objs).build())).unwrap();
    let shape = Shape::Redact { quads: vec![rect_quad([0.0, 200.0, 300.0, 300.0])], overlay: String::new(), look: Default::default() };
    let style = Style::default_for(&shape);
    add_annotation(&mut doc, &NewAnnotation { page: 0, shape, style, contents: String::new(), author: "Synthetic test".into() }, &Meta::default())
        .unwrap();
    doc
}

/// One page whose `/Properties` holds `entries` entries, drawing `ops`.
fn properties(entries: usize, ops: &[u8]) -> Document {
    let lists: String = (0..entries).map(|i| format!("/M{i} << /Lang (en) >> ")).collect();
    page(&format!("/Properties << {lists}>>"), ops, Vec::new())
}

/// One page whose property list `/M0` is object 5, which gives its `MCID` as object 6 and holds
/// 400,000 more entries: larger than the document keeps, so that reading it again parses it again,
/// all of it, once another object has been read since. The page draws `ops`.
fn oversized(ops: &[u8]) -> Document {
    let more: String = (0..400_000).map(|i| format!("/J{i} /DeviceGray ")).collect();
    page("/Properties << /M0 5 0 R >>", ops, vec![format!("<< /MCID 6 0 R {more}>>").into_bytes(), b"0".to_vec()])
}

/// The tests' lock: they run one at a time, so that none is timed while another works.
static SERIAL: Mutex<()> = Mutex::new(());

fn one_at_a_time() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How long redacting a document `make` builds takes, the least of three runs.
fn redaction(make: impl Fn() -> Document) -> Duration {
    (0..3)
        .map(|_| {
            let mut doc = make();
            let start = Instant::now();
            let report = apply(&mut doc, None).unwrap();
            let took = start.elapsed();
            assert_eq!(report.glyphs, 6, "the word under the mark goes");
            took
        })
        .min()
        .unwrap_or_default()
}

/// Whether `many` took about as long as `one`.
fn about_as_long(many: Duration, one: Duration) -> bool {
    many < one * 10 + Duration::from_secs(1)
}

/// Written when the tags cleanup resolved the page's `/Properties` at every named `BDC`: 20,000 of
/// them with 5,000 entries took 28 s against 0.07 s with one entry (optimized). Each `BDC` here
/// names another entry, so that it is the lists, not each name's answer, that are read once.
#[test]
fn named_property_lists_are_read_once_for_a_page() {
    let _serial = one_at_a_time();
    let ops: Vec<u8> = (0..5_000).map(|i| format!("/P /M{i} BDC EMC ")).collect::<String>().into_bytes();
    let (one, many) = (redaction(|| properties(1, &ops)), redaction(|| properties(5_000, &ops)));
    assert!(about_as_long(many, one), "5,000 entries {many:?}, one {one:?}");
}

/// Written when the tags cleanup read a named property list's `MCID` by reference at each `BDC`: a
/// list larger than the document keeps was parsed again at each, 50 of them taking 12.3 s against
/// 0.44 s with the `MCID` given directly (optimized).
#[test]
fn an_oversized_property_list_is_read_once_for_a_stream() {
    let _serial = one_at_a_time();
    let (one, many) = (redaction(|| oversized(b"/P /M0 BDC EMC ")), redaction(|| oversized(&b"/P /M0 BDC EMC ".repeat(100))));
    assert!(about_as_long(many, one), "100 operators {many:?}, one {one:?}");
}
