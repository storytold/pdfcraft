//! Contributor-original synthetic pages for the special colourant names of a Separation colour
//! space (ISO 32000-2 §8.6.6.4): `/All` paints every colourant of the output device at once, so
//! on screen a tint t is the gray 1 − t whatever the alternate space and tint transform say;
//! `/None` paints nothing; any other name goes through its alternate space.
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RenderedPage};
use std::sync::Arc;

/// A tint transform into DeviceCMYK that gives cyan: tint t → (t, 0, 0, 0). Through it a tint
/// draws light or full cyan, never a neutral gray, so a gray result proves it was not used.
const CYAN: &str = "<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [1 0 0 0] /N 1 >>";

/// A PDF of the given objects (object 1 the catalog), with a cross-reference table. Each object
/// is its bytes: a stream's data need not be text.
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

/// A 300 × 100 pt page drawing `content` with `resources`, then `extra` as objects 5 onwards.
fn page(content: &str, resources: &str, extra: &[Vec<u8>]) -> Vec<u8> {
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 100] /Contents 4 0 R /Resources {resources} >>").into_bytes(),
        stream("", content.as_bytes()),
    ];
    objects.extend_from_slice(extra);
    pdf(&objects)
}

fn render(pdf: Vec<u8>) -> RenderedPage {
    let mut renderer = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
    let out = renderer.render(RenderRequest { page: 0, scale: 1.0, ..Default::default() });
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!((out.width, out.height), (300, 100));
    out
}

/// The RGB of the pixel at (`x`, `y`) pt from the page's top left.
fn rgb(out: &RenderedPage, x: usize, y: usize) -> [u8; 3] {
    let at = (y * out.width as usize + x) * 4;
    let pixel = out.rgba.get(at..at + 3).expect("the pixel lies on the page");
    [pixel[0], pixel[1], pixel[2]]
}

/// `got` is the neutral gray `want`, give or take one level of rounding.
fn assert_gray(got: [u8; 3], want: u8, what: &str) {
    assert!(got.iter().all(|c| c.abs_diff(want) <= 1), "{what}: got {got:?}, want gray {want}");
}

/// Three boxes filled with `cs` at tints 0, 0.5 and 1, left to right.
fn three_tints(space: &str) -> RenderedPage {
    let content = "/CS0 cs 0 scn 0 0 100 100 re f 0.5 scn 100 0 100 100 re f 1 scn 200 0 100 100 re f";
    render(page(content, "<< /ColorSpace << /CS0 5 0 R >> >>", &[space.as_bytes().to_vec(), CYAN.as_bytes().to_vec()]))
}

#[test]
fn separation_all_fills_paint_the_complemented_tint_as_gray() {
    let out = three_tints("[/Separation /All /DeviceCMYK 6 0 R]");
    assert_gray(rgb(&out, 50, 50), 255, "tint 0 paints no colourant: white");
    assert_gray(rgb(&out, 150, 50), 128, "tint 0.5 paints half of every colourant: 50% gray");
    assert_gray(rgb(&out, 250, 50), 0, "tint 1 paints every colourant: black");
}

#[test]
fn separation_all_strokes_paint_the_complemented_tint_as_gray() {
    let content = "/CS0 CS 0.25 SCN 40 w 0 50 m 300 50 l S";
    let out =
        render(page(content, "<< /ColorSpace << /CS0 5 0 R >> >>", &[b"[/Separation /All /DeviceCMYK 6 0 R]".to_vec(), CYAN.as_bytes().to_vec()]));
    assert_gray(rgb(&out, 150, 50), 191, "a stroke at tint 0.25: 75% gray");
}

#[test]
fn separation_none_paints_nothing() {
    // A red background, so "nothing" is told apart from white.
    let content = "1 0 0 rg 0 0 300 100 re f /CS0 cs 0 scn 0 0 100 100 re f 0.5 scn 100 0 100 100 re f 1 scn 200 0 100 100 re f";
    let out =
        render(page(content, "<< /ColorSpace << /CS0 5 0 R >> >>", &[b"[/Separation /None /DeviceCMYK 6 0 R]".to_vec(), CYAN.as_bytes().to_vec()]));
    for x in [50, 150, 250] {
        assert_eq!(rgb(&out, x, 50), [255, 0, 0], "a /None fill at x = {x} leaves the background");
    }
}

#[test]
fn a_named_separation_still_paints_through_its_alternate() {
    let out = three_tints("[/Separation /Cyan /DeviceCMYK 6 0 R]");
    let [white, half, full] = [rgb(&out, 50, 50), rgb(&out, 150, 50), rgb(&out, 250, 50)];
    assert_gray(white, 255, "tint 0 of a named colourant");
    for (what, colour) in [("tint 0.5", half), ("tint 1", full)] {
        // Cyan: red well below green and blue.
        assert!(colour[0] < colour[1] && colour[0] < colour[2] && colour[2] > 200, "{what} of /Cyan is cyan: {colour:?}");
    }
    assert!(full[0] < half[0], "full cyan is deeper than half: {full:?} against {half:?}");
}

#[test]
fn a_device_n_space_naming_all_still_paints_through_its_alternate() {
    // §8.6.6.4 gives All its meaning in a Separation space; a DeviceN space keeps its alternate.
    let out = three_tints("[/DeviceN [/All] /DeviceCMYK 6 0 R]");
    let half = rgb(&out, 150, 50);
    assert!(half[0] < half[1] && half[0] < half[2], "DeviceN [/All] at 0.5 draws its tint transform's cyan: {half:?}");
}

#[test]
fn separation_all_images_paint_each_sample_as_the_complemented_gray() {
    // Three 8-bit samples: tints 0, 128/255 and 1.
    let data = [0u8, 128, 255];
    let image =
        stream("/Type /XObject /Subtype /Image /Width 3 /Height 1 /ColorSpace [/Separation /All /DeviceCMYK 6 0 R] /BitsPerComponent 8", &data);
    let out = render(page("q 300 0 0 100 0 0 cm /Im Do Q", "<< /XObject << /Im 5 0 R >> >>", &[image, CYAN.as_bytes().to_vec()]));
    assert_gray(rgb(&out, 50, 50), 255, "sample 0");
    assert_gray(rgb(&out, 150, 50), 127, "sample 128");
    assert_gray(rgb(&out, 250, 50), 0, "sample 255");
}

#[test]
fn separation_all_shadings_paint_the_complemented_gray() {
    // An axial shading from tint 0 at x = 0 to tint 1 at x = 300, painted with `sh`.
    let shading = "<< /ShadingType 2 /ColorSpace [/Separation /All /DeviceCMYK 6 0 R] /Coords [0 0 300 0] /Function << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >> /Extend [true true] >>";
    let out = render(page("/Sh sh", "<< /Shading << /Sh 5 0 R >> >>", &[shading.as_bytes().to_vec(), CYAN.as_bytes().to_vec()]));
    for (x, want) in [(1usize, 254u8), (150, 128), (298, 1)] {
        let got = rgb(&out, x, 50);
        assert!(got.iter().all(|c| c.abs_diff(want) <= 3), "shading at x = {x}: got {got:?}, want gray near {want}");
    }
}
