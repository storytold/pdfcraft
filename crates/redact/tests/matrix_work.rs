//! Work regression for a form's `/Matrix`, which the redaction walk reads at every `Do` of the
//! form: an array of any length costs no more than one of seven entries, written directly or by
//! reference. A test binary of its own, so that none of the crate's other tests shares its process.
//! The test compares two timings of its own, the least of three runs each, not a fixed time.

use std::sync::Arc;
use std::time::{Duration, Instant};

use pdfcraft_annot::{Meta, NewAnnotation, Shape, Style, add_annotation, rect_quad};
use pdfcraft_cos::Document;
use pdfcraft_redact::apply;
use pdfcraft_testkit::{Pdf, stream_obj};

/// One page drawing a form `draws` times under a mark, the form's `/Matrix` an array of `entries`
/// zeros, written in its dictionary or, `by_reference`, as object 6.
fn document(draws: usize, entries: usize, by_reference: bool) -> Document {
    let zeros = format!("[{}]", vec!["0"; entries].join(" "));
    let matrix = if by_reference { "6 0 R".to_string() } else { zeros.clone() };
    let objs = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /XObject << /Fm 5 0 R >> >> >>".to_vec(),
        stream_obj("", "/Fm Do ".repeat(draws).as_bytes()),
        stream_obj(&format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Matrix {matrix}"), b"0 0 10 10 re f"),
        zeros.into_bytes(),
    ];
    let mut doc = Document::open(Arc::new(Pdf::new().objs(objs).build())).unwrap();
    let shape = Shape::Redact { quads: vec![rect_quad([0.0, 0.0, 300.0, 300.0])], overlay: String::new(), look: Default::default() };
    let style = Style::default_for(&shape);
    add_annotation(&mut doc, &NewAnnotation { page: 0, shape, style, contents: String::new(), author: "Synthetic test".into() }, &Meta::default())
        .unwrap();
    doc
}

/// How long redacting `document(draws, entries, by_reference)` takes, the least of three runs. A
/// matrix that isn't six numbers is refused.
fn redaction(draws: usize, entries: usize, by_reference: bool) -> Duration {
    (0..3)
        .map(|_| {
            let mut doc = document(draws, entries, by_reference);
            let start = Instant::now();
            let refused = apply(&mut doc, None).is_err();
            let took = start.elapsed();
            assert!(refused, "a /Matrix of {entries} entries is refused");
            took
        })
        .min()
        .unwrap_or_default()
}

/// Written when the walk resolved and copied every entry before checking there were six: 4,000
/// draws of a form whose `/Matrix` is 4,000 entries long read sixteen million entries.
#[test]
fn a_long_form_matrix_costs_no_more_than_a_short_one() {
    for by_reference in [false, true] {
        let (short, long) = (redaction(4_000, 7, by_reference), redaction(4_000, 4_000, by_reference));
        assert!(long < short * 3 + Duration::from_millis(50), "by reference: {by_reference}: long {long:?}, short {short:?}");
    }
}
