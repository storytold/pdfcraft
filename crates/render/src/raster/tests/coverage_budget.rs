//! A page that paints the same content over and over runs out of rasterizer coverage: what is
//! past it is left out and said to be, rather than panicking once a strip's alpha index no longer
//! fits (PDFBOX-5415's map page drew one form 2^22 times and panicked at 7 GB).

use super::*;
use std::sync::Mutex;

/// A page whose form `F0` draws `F1` twice, `F1` draws `F2` twice, and so on `levels` deep; the
/// last form fills a circle. The page draws `F0` once, so the circle is filled 2^`levels` times.
fn doubling_forms(levels: usize) -> Vec<u8> {
    let mut objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        // Object 4 is the page's content; forms start at 5.
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Resources << /XObject << /F0 5 0 R >> >> >>".into(),
        String::new(),
    ];
    let content = "/F0 Do";
    objs[3] = format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len());
    let circle = "0 0 1 rg 100 160 m 133 160 160 133 160 100 c 160 67 133 40 100 40 c 67 40 40 67 40 100 c 40 133 67 160 100 160 c f";
    for level in 0..=levels {
        let (body, resources) = if level == levels {
            (circle.to_string(), String::new())
        } else {
            ("/F Do /F Do".to_string(), format!("/Resources << /XObject << /F {} 0 R >> >>", 6 + level))
        };
        objs.push(format!("<< /Type /XObject /Subtype /Form /BBox [0 0 200 200] {resources} /Length {} >>\nstream\n{body}\nendstream", body.len()));
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// Renders page 1 with the rasterizer kept to `max_coverage` values, returning its pixels and
/// whether it said content was left out.
fn render_kept_to(pdf: Vec<u8>, max_coverage: Option<usize>) -> (Vec<u8>, bool) {
    let parsed = Pdf::new(Arc::new(pdf)).unwrap();
    let truncated = Arc::new(Mutex::new(false));
    let seen = truncated.clone();
    let settings = InterpreterSettings {
        warning_sink: Arc::new(move |w| {
            if matches!(w, InterpreterWarning::ContentTruncated) {
                *seen.lock().unwrap() = true;
            }
        }),
        ..Default::default()
    };
    let pixmap =
        hayro::render(&parsed.pages()[0], &RenderCache::new(), &settings, &RenderSettings { bg_color: WHITE, max_coverage, ..Default::default() });
    let cut = *truncated.lock().unwrap();
    (pixmap.data_as_u8_slice().to_vec(), cut)
}

#[test]
fn a_page_past_its_coverage_leaves_the_rest_out_and_says_so() {
    // 2^10 circles, each storing its edge tiles' coverage.
    let pdf = doubling_forms(10);
    let (whole, whole_cut) = render_kept_to(pdf.clone(), None);
    assert!(!whole_cut, "within the limit nothing is left out");
    let blue = |px: &[u8]| px.as_chunks::<4>().0.iter().filter(|p| p[2] > 200 && p[0] < 50).count();
    assert!(blue(&whole) > 10_000, "the circle is drawn");

    // Room for a few circles only: the rest are left out, the page still renders, and says so.
    let (kept, cut) = render_kept_to(pdf, Some(200_000));
    assert!(cut, "the page says content was left out");
    assert_eq!(kept.len(), whole.len());
    assert!(blue(&kept) > 10_000, "the circles drawn before the limit stay: {}", blue(&kept));
}
