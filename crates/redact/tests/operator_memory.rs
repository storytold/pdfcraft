//! Memory regressions for the streams a redaction reads besides its walk: the forms it follows
//! through resources, and what the Type 3 cleanup scans and reads. Their operators are read one
//! at a time, so a stream of millions of them is never held whole. A test binary of its own, so
//! that the one test is all the process runs while its peak is measured.

use std::sync::Arc;

use pdfcraft_annot::{Meta, NewAnnotation, Shape, Style, add_annotation, rect_quad};
use pdfcraft_cos::{Document, ObjRef, Object, Stream};
use pdfcraft_redact::apply;
use pdfcraft_testkit::{Pdf, stream_obj};

const IMAGE: &str = "/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8";

/// 32 MiB of `n` operators: 16 million of them, eight times the walk's limit. Held as parsed
/// operators, at about 120 bytes each, nearly 2 GiB.
fn operators() -> Vec<u8> {
    b"n\n".repeat(16 << 20)
}

/// `form` with `content`, compressed.
fn compress(doc: &mut Document, form: ObjRef, content: &[u8]) {
    let Object::Stream(s) = &*doc.get(form) else { panic!("{form:?} is a stream") };
    let dict = s.dict.clone();
    doc.set(form, Object::Stream(Stream::flate(dict, content)));
}

fn mark(doc: &mut Document) {
    let shape = Shape::Redact { quads: vec![rect_quad([0.0, 200.0, 300.0, 300.0])], overlay: String::new(), look: Default::default() };
    let style = Style::default_for(&shape);
    add_annotation(doc, &NewAnnotation { page: 0, shape, style, contents: String::new(), author: "Synthetic test".into() }, &Meta::default())
        .unwrap();
}

/// How much the process's peak resident memory grows, in MiB, while `f` runs. `None` where the
/// peak can't be reset (`/proc/self/clear_refs` is Linux's).
fn peak_growth<T>(f: impl FnOnce() -> T) -> Option<(T, u64)> {
    fn peak_kib() -> Option<u64> {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        status.lines().find_map(|l| l.strip_prefix("VmHWM:"))?.split_whitespace().next()?.parse().ok()
    }
    std::fs::write("/proc/self/clear_refs", "5").ok()?;
    let before = peak_kib()?;
    let out = f();
    Some((out, peak_kib()?.saturating_sub(before) >> 10))
}

/// A page that draws an image under the mark, a form `form` with resources of its own outside it,
/// and a glyph of a Type 3 font outside it, whose program is `glyph`: the Type 3 cleanup reads
/// every page and form, and the programs of the glyphs it keeps.
fn type3_page(form: &[u8], glyph: &[u8]) -> Document {
    let mut doc = Document::open(Arc::new(
        Pdf::new()
            .objs(vec![
                b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
                b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
                b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /T3 7 0 R >> /XObject << /Im1 5 0 R /Fm 6 0 R >> >> >>"
                    .to_vec(),
                stream_obj("", b"q 50 0 0 50 20 230 cm /Im1 Do Q /Fm Do BT /T3 10 Tf 10 10 Td (a) Tj ET"),
                stream_obj(IMAGE, b"o"),
                stream_obj("/Type /XObject /Subtype /Form /BBox [0 0 100 100] /Resources << >>", b""),
                b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /a 8 0 R >> /Encoding << /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [1000] /Resources << >> >>"
                    .to_vec(),
                stream_obj("", b""),
            ])
            .build(),
    ))
    .unwrap();
    compress(&mut doc, ObjRef::new(6, 0), form);
    compress(&mut doc, ObjRef::new(8, 0), glyph);
    mark(&mut doc);
    doc
}

/// Four reads of a stream of 16 million operators:
/// - the page's check of what it still draws walks a form without resources of its own outside
///   the mark, and refuses at 2,000,001 operators;
/// - the unlinking check of a page nobody marked, which shares the marked page's resources,
///   looks through such a form for the image the marked page removed, and finds no `Do`;
/// - the Type 3 cleanup scans a form with resources of its own that nothing else reads, for the
///   glyphs it shows;
/// - the Type 3 cleanup reads the program of a glyph it keeps, for the resources it uses.
///
/// Each parsed every operator first: the peak grew by 4,415, 2,526, 2,526 and 2,494 MiB. Now
/// the first holds at most one operator past the walk's limit, and the others none.
#[test]
fn the_forms_followed_through_resources_are_read_one_operator_at_a_time() {
    // The page's check: an image under the mark, and the form outside it.
    let mut doc = Document::open(Arc::new(
        Pdf::new()
            .objs(vec![
                b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
                b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
                b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /XObject << /Im1 5 0 R /Fm 6 0 R >> >> >>"
                    .to_vec(),
                stream_obj("", b"q 50 0 0 50 20 230 cm /Im1 Do Q /Fm Do"),
                stream_obj(IMAGE, b"o"),
                stream_obj("/Type /XObject /Subtype /Form /BBox [0 0 100 100]", b""),
            ])
            .build(),
    ))
    .unwrap();
    compress(&mut doc, ObjRef::new(6, 0), &operators());
    mark(&mut doc);
    let Some((result, grew)) = peak_growth(|| apply(&mut doc, None)) else { return };
    assert!(result.is_err(), "the walk refuses past 2,000,000 operators: {result:?}");
    assert!(grew < 1024, "the page's check grew the peak by {grew} MiB");

    // The unlinking check: two pages share the /Pages node's resources; the second draws the form.
    let mut doc = Document::open(Arc::new(
        Pdf::new()
            .objs(vec![
                b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
                b"<< /Type /Pages /Kids [6 0 R 7 0 R] /Count 2 /Resources << /XObject << /Im1 5 0 R /Fm 3 0 R >> >> >>".to_vec(),
                stream_obj("/Type /XObject /Subtype /Form /BBox [0 0 100 100]", b""),
                stream_obj("", b"q 50 0 0 50 20 230 cm /Im1 Do Q"),
                stream_obj(IMAGE, b"o"),
                b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R >>".to_vec(),
                b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 8 0 R >>".to_vec(),
                stream_obj("", b"/Fm Do"),
            ])
            .build(),
    ))
    .unwrap();
    compress(&mut doc, ObjRef::new(3, 0), &operators());
    mark(&mut doc);
    let Some((result, grew)) = peak_growth(|| apply(&mut doc, Some(&[0]))) else { return };
    assert_eq!(result.map(|r| r.images_removed).ok(), Some(1), "the marked page's image goes");
    assert!(grew < 1024, "the unlinking check grew the peak by {grew} MiB");

    let mut doc = type3_page(&operators(), b"1000 0 d0");
    let Some((result, grew)) = peak_growth(|| apply(&mut doc, None)) else { return };
    assert_eq!(result.map(|r| r.images_removed).ok(), Some(1), "the Type 3 scan: the image goes");
    assert!(grew < 1024, "the Type 3 cleanup's scan grew the peak by {grew} MiB");

    let mut glyph = b"1000 0 d0\n".to_vec();
    glyph.extend(operators());
    let mut doc = type3_page(b"", &glyph);
    let Some((result, grew)) = peak_growth(|| apply(&mut doc, None)) else { return };
    assert_eq!(result.map(|r| r.images_removed).ok(), Some(1), "the glyph program: the image goes");
    assert!(grew < 1024, "the Type 3 cleanup's read of a glyph program grew the peak by {grew} MiB");
}
