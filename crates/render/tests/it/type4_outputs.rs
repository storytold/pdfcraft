//! Contributor-original synthetic pages for the outputs of a Type 4 (PostScript calculator)
//! function (ISO 32000-2 §7.10.5): the outputs are the values left on top of the operand stack,
//! as many as `/Range` has pairs. A program that leaves more values than that keeps the top
//! ones (hayro-interpret patch 115, storytold/craft-libs#2698); one that leaves fewer fails like
//! any evaluation that cannot finish.
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RenderedPage};
use std::sync::Arc;

/// A PDF of the given objects (object 1 the catalog), with a cross-reference table.
fn pdf(objects: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        bytes.extend_from_slice(object);
        bytes.extend_from_slice(b"\nendobj\n");
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    bytes
}

fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
    let mut bytes = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
    bytes.extend_from_slice(data);
    bytes.extend_from_slice(b"\nendstream");
    bytes
}

/// A 100 × 100 pt page filled at tint 0.6 of `[/Separation /Probe <alternate> f]`, where `f`
/// is a Type 4 function with one input, `outputs` output ranges of [0 1] and `program`.
fn fill(alternate: &str, outputs: usize, program: &str) -> RenderedPage {
    let range = vec!["0 1"; outputs].join(" ");
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R \
             /Resources << /ColorSpace << /CS0 [/Separation /Probe {alternate} 5 0 R] >> >> >>"
        )
        .into_bytes(),
        stream("", b"/CS0 cs 0.6 scn 0 0 100 100 re f"),
        stream(&format!("/FunctionType 4 /Domain [0 1] /Range [{range}]"), program.as_bytes()),
    ];
    let mut renderer = PageRenderer::new(Arc::new(pdf(&objects)), RenderConfig::default());
    let out = renderer.render(RenderRequest { page: 0, scale: 1.0, ..Default::default() });
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!((out.width, out.height), (100, 100));
    out
}

/// The RGB of the pixel in the middle of the page.
fn middle(out: &RenderedPage) -> [u8; 3] {
    let at = (50 * out.width as usize + 50) * 4;
    let pixel = out.rgba.get(at..at + 3).expect("the pixel lies on the page");
    [pixel[0], pixel[1], pixel[2]]
}

fn assert_rgb(got: [u8; 3], want: [u8; 3], what: &str) {
    assert!(got.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 1), "{what}: got {got:?}, want {want:?}");
}

#[test]
fn extra_values_below_the_outputs_are_ignored() {
    // `0 exch 0 exch 0` leaves 0 0 t 0 for three outputs, as in the veraPDF PDF/A-4 6.2.4.4
    // t01 files: the top three, (0, t, 0), are green. The bottom three were blue.
    assert_rgb(middle(&fill("/DeviceRGB", 3, "{0 exch 0 exch 0}")), [0, 153, 0], "four values for three outputs");
    // 1 0 0 | 0 1 0: green from the top three, not red from the bottom three.
    assert_rgb(middle(&fill("/DeviceRGB", 3, "{pop 1 0 0 0 1 0}")), [0, 255, 0], "six values for three outputs");
    // 0.2 0.5 | 0.8 into DeviceGray: one output, the top value.
    assert_rgb(middle(&fill("/DeviceGray", 1, "{pop 0.2 0.5 0.8}")), [204, 204, 204], "three values for one output");
}

#[test]
fn a_program_leaving_exactly_the_outputs_is_unchanged() {
    assert_rgb(middle(&fill("/DeviceRGB", 3, "{pop 0 1 0}")), [0, 255, 0], "three values for three outputs");
    assert_rgb(middle(&fill("/DeviceRGB", 3, "{dup 0}")), [153, 153, 0], "the input kept as two outputs");
}

#[test]
fn a_program_leaving_too_few_values_falls_back_to_the_initial_colour() {
    // ISO 32000-2 §7.10.5.3 calls this an error, and readers disagree on what to draw. The
    // tint transform fails, and the colour falls back to the alternate space's initial colour,
    // black, as for any failed evaluation (hayro-interpret patch 55).
    for program in ["{pop 0.5}", "{pop 1 0.5}", "{pop}"] {
        assert_rgb(middle(&fill("/DeviceRGB", 3, program)), [0, 0, 0], program);
    }
}
