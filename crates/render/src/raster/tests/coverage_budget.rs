//! A page that paints the same content over and over runs out of rasterizer coverage: what is
//! past it is left out and said to be, rather than panicking once a strip's alpha index no longer
//! fits (PDFBOX-5415's map page drew one form 2^22 times and panicked at 7 GB).

use super::*;
use std::sync::Mutex;

/// A blue circle, filled.
const FILLED_CIRCLE: &str = "0 0 1 rg 100 160 m 133 160 160 133 160 100 c 160 67 133 40 100 40 c 67 40 40 67 40 100 c 40 133 67 160 100 160 c f";

/// A page whose form `F0` draws `F1` twice, `F1` draws `F2` twice, and so on `levels` deep; the
/// last form fills a circle. The page draws `F0` once, so the circle is filled 2^`levels` times.
fn doubling_forms(levels: usize) -> Vec<u8> {
    doubling_forms_then(levels, FILLED_CIRCLE, "")
}

/// [`doubling_forms`] with the last form painting `leaf` instead, and the page painting `after`
/// once the forms are done.
fn doubling_forms_then(levels: usize, leaf: &str, after: &str) -> Vec<u8> {
    let mut objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        // Object 4 is the page's content; forms start at 5.
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Resources << /XObject << /F0 5 0 R >> >> >>".into(),
        String::new(),
    ];
    let content = format!("/F0 Do {after}");
    objs[3] = format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len());
    for level in 0..=levels {
        let (body, resources) = if level == levels {
            (leaf.to_string(), String::new())
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

/// Pixels of `px` that are mostly red.
fn red(px: &[u8]) -> usize {
    px.as_chunks::<4>().0.iter().filter(|p| p[0] > 200 && p[2] < 50).count()
}

/// The page paints `leaf` 2^10 times through forms and then `marker` (red, away from the leaf's
/// drawing) once, both through the same rasterizer path. Kept to 200,000 coverage values, the
/// marker is past the limit: it is left out and the page says so. Unlimited, it is drawn.
fn leaves_out_past_the_limit(leaf: &str, marker: &str) {
    let pdf = doubling_forms_then(10, leaf, marker);
    let (whole, whole_cut) = render_kept_to(pdf.clone(), None);
    assert!(!whole_cut, "within the limit nothing is left out");
    assert!(red(&whole) > 50, "unlimited, the marker is drawn: {}", red(&whole));

    let (kept, cut) = render_kept_to(pdf, Some(200_000));
    assert!(cut, "the page says content was left out");
    assert_eq!(kept.len(), whole.len());
    assert_eq!(red(&kept), 0, "the marker, past the limit, is left out");
}

#[test]
fn strokes_past_the_coverage_are_left_out() {
    // A stroked circle and a stroked curve: neither is a rectangle, so both go through
    // `stroke_path`, and the page's only coverage is strokes.
    leaves_out_past_the_limit(
        "0 0 1 RG 4 w 100 160 m 133 160 160 133 160 100 c 160 67 133 40 100 40 c 67 40 40 67 40 100 c 40 133 67 160 100 160 c S",
        "1 0 0 RG 6 w 170 10 m 180 30 190 10 195 30 c S",
    );
}

#[test]
fn fast_rectangles_past_the_coverage_are_left_out() {
    // Axis-aligned rectangles fill through `fill_rect_fast`, and are the page's only coverage.
    leaves_out_past_the_limit("0 0 1 rg 40.5 40.5 119 119 re f", "1 0 0 rg 170.5 10.5 20 20 re f");
}

#[test]
fn layer_clips_past_the_coverage_are_left_out() {
    // hayro pushes no clipped layers (its clips go through `push_clip_path`), so the layer clip
    // is driven through the vello_cpu context directly, with clips as the scene's only coverage.
    use hayro::vello_cpu::kurbo::{Circle, Shape};
    use hayro::vello_cpu::{Pixmap, RenderContext, Resources};

    let mut ctx = RenderContext::new(100, 100);
    // Room while fewer than 30,000 - 2 x 100 x 100 = 10,000 values are stored.
    ctx.set_max_coverage(30_000);
    let clip = Circle::new((50.0, 50.0), 40.0).to_path(0.1);
    for _ in 0..1_000 {
        ctx.push_clip_layer(&clip);
        ctx.pop_layer();
    }
    assert!(ctx.coverage_full(), "the clips past the limit were left out, and that is said");
    // The scene still rasterizes: the layers left unclipped have nothing drawn into them.
    let mut pixmap = Pixmap::new(100, 100);
    ctx.flush();
    ctx.render_to_pixmap(&mut Resources::new(), &mut pixmap);

    ctx.reset();
    assert!(!ctx.coverage_full(), "a reset scene starts with room");
}
