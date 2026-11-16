//! Memory regressions for the pattern names the redaction walk records as painted, so that the
//! page's pattern cleanup keeps what still paints, and for the names a graphics state holds. A long
//! name selected once and painted with again and again is recorded once, not once for each paint,
//! nor once for each draw of a form, and a state saved again and again shares it. A test binary
//! of its own, so that the one test is all the process runs while its peak is measured.

use std::sync::Arc;

use pdfcraft_annot::{Meta, NewAnnotation, Shape, Style, add_annotation, rect_quad};
use pdfcraft_cos::Document;
use pdfcraft_redact::apply;
use pdfcraft_testkit::{Pdf, stream_obj};

const PATTERN: &str = "/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 10 /YStep 10 /Resources << >>";

/// A pattern name of 64 KiB.
fn long_name() -> String {
    format!("P{}", "x".repeat((64 << 10) - 1))
}

/// A page of `content`, drawing `/Fm` (object 5, `form`, without resources of its own, over the
/// mark), with `resources` besides, marked across its top.
fn page(content: &[u8], resources: &str, form: &[u8], more: Vec<Vec<u8>>) -> Document {
    let mut objs = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /XObject << /Fm 5 0 R >> {resources} >> >>")
            .into_bytes(),
        stream_obj("", content),
        stream_obj("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", form),
    ];
    objs.extend(more);
    let mut doc = Document::open(Arc::new(Pdf::new().objs(objs).build())).unwrap();
    let shape = Shape::Redact { quads: vec![rect_quad([0.0, 200.0, 300.0, 300.0])], overlay: String::new(), look: Default::default() };
    let style = Style::default_for(&shape);
    add_annotation(&mut doc, &NewAnnotation { page: 0, shape, style, contents: String::new(), author: "Synthetic test".into() }, &Meta::default())
        .unwrap();
    doc
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

/// A pattern whose name is 64 KiB long, beside the mark:
/// - selected by the page and painted with 4,096 times;
/// - selected by the page, then painted with once in each of 4,096 draws of a form without
///   resources of its own over the mark;
/// - selected and painted with once in each of 3,900 draws of such a form, which the page's
///   allowance decodes;
/// - selected as fill and stroke, then saved 4,096 times with `q`;
/// - and 64 pattern names of 1 KiB, where a Type 3 font without resources of its own, which may
///   paint with any of them, is shown 4,096 times.
///
/// And a font whose name is 256 KiB long, set before 1,024 `q`, as the proof's text reader reads
/// it when the redaction is captured for its proof.
///
/// Each recorded or copied the name again for each paint, draw, glyph shown or state saved: from
/// about 180 to 510 MiB each. Now each name is recorded once, and shared.
#[test]
fn a_pattern_name_painted_again_and_again_is_recorded_once() {
    let name = long_name();
    let resources = format!("/Pattern << /{name} 6 0 R >>");
    let cell = || stream_obj(PATTERN, b"0 0 5 5 re f");
    let shapes: [(&str, Vec<u8>, Vec<u8>); 4] = [
        ("painted by the page", [format!("/Pattern cs /{name} scn ").into_bytes(), b"0 0 1 1 re f\n".repeat(4096)].concat(), Vec::new()),
        (
            "painted in each draw of a form",
            [format!("/Pattern cs /{name} scn ").into_bytes(), b"/Fm Do\n".repeat(4096)].concat(),
            b"0 0 1 1 re f".to_vec(),
        ),
        ("selected in each draw of a form", b"/Fm Do\n".repeat(3900), format!("/Pattern cs /{name} scn 0 0 1 1 re f").into_bytes()),
        (
            "saved with q",
            [
                format!("/Pattern cs /{name} scn /Pattern CS /{name} SCN ").into_bytes(),
                b"q ".repeat(4096),
                b"0 0 1 1 re f ".to_vec(),
                b"Q ".repeat(4096),
            ]
            .concat(),
            Vec::new(),
        ),
    ];
    let mut grown = Vec::new();
    for (label, content, form) in shapes {
        let mut doc = page(&content, &resources, &form, vec![cell()]);
        let Some((result, grew)) = peak_growth(|| apply(&mut doc, None)) else { return };
        assert!(result.is_ok(), "{label}: {result:?}");
        grown.push((label, grew));
    }

    let names: String = (0..64).map(|i| format!("/Q{i}{} 6 0 R ", "y".repeat(1 << 10))).collect();
    let type3 = b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /a 8 0 R >> /Encoding << /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [1000] >>";
    let content = [b"BT /T3 10 Tf 10 10 Td ".to_vec(), b"(a) Tj ".repeat(4096), b"ET".to_vec()].concat();
    let mut doc = page(
        &content,
        &format!("/Pattern << {names}>> /Font << /T3 7 0 R >>"),
        b"",
        vec![cell(), type3.to_vec(), stream_obj("", b"1000 0 d0 0 0 1000 1000 re f")],
    );
    let Some((result, grew)) = peak_growth(|| apply(&mut doc, None)) else { return };
    assert!(result.is_ok(), "a Type 3 font: {result:?}");
    grown.push(("a Type 3 font shown", grew));

    let font = format!("F{}", "x".repeat((256 << 10) - 1));
    let content = [format!("BT /{font} 12 Tf ET ").into_bytes(), b"q ".repeat(1024), b"0 0 1 1 re f ".to_vec(), b"Q ".repeat(1024)].concat();
    let doc = page(&content, "", b"", Vec::new());
    let Some((snapshot, grew)) = peak_growth(|| pdfcraft_redact::Snapshot::capture(&doc, None)) else { return };
    assert!(snapshot.is_ok(), "the proof's capture");
    grown.push(("a font name saved with q, for the proof", grew));
    assert!(grown.iter().all(|(_, grew)| *grew < 64), "the peak grew by (MiB): {grown:?}");
}
