//! A luminosity soft mask takes its values from the gray of its group's colour (ISO 32000-2
//! §11.5.3). In a `/DeviceCMYK` group that gray is `1 - min(1, 0.3 c + 0.59 m + 0.11 y + k)`
//! (§10.4.2.3), so 100% K masks fully, whatever profile CMYK is displayed through.

use std::sync::Arc;

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind};

/// The page's grey backdrop, `0.5 g`, as the renderer writes it.
const BACKDROP: f64 = 128.0;

fn pdf(objects: &[String]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    out
}

fn stream(dict: &str, content: &str) -> String {
    format!("<< {dict} /Length {} >>\nstream\n{content}\nendstream", content.len())
}

fn render(bytes: Vec<u8>) -> (Vec<u8>, u32) {
    let mut renderer = PageRenderer::new(Arc::new(bytes), RenderConfig::default());
    let page = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, preview: false });
    assert!(page.error.is_none(), "{:?}", page.error);
    (page.rgba.to_vec(), page.width)
}

fn pixel(rgba: &[u8], width: u32, x: u32, y: u32) -> [u8; 3] {
    let offset = usize::try_from((y * width + x) * 4).unwrap();
    [rgba[offset], rgba[offset + 1], rgba[offset + 2]]
}

/// Red filled over the grey backdrop under a luminosity mask whose group, in `space`, paints its
/// left half with `paint`; its right half is the backdrop `/BC` (`bc`, when given). Returns the
/// mask values read back from the left and the right half.
fn mask_values(space: &str, paint: &str, bc: Option<&str>) -> (f64, f64) {
    let bc = bc.map(|bc| format!("/BC {bc}")).unwrap_or_default();
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R /Resources << /ExtGState << /M 5 0 R >> >> >>".to_string(),
        stream("", "0.5 g 0 0 100 100 re f /M gs 1 0 0 rg 0 0 100 100 re f"),
        format!("<< /Type /ExtGState /SMask << /Type /Mask /S /Luminosity /G 6 0 R {bc} >> >>"),
        stream(
            &format!("/Type /XObject /Subtype /Form /BBox [0 0 100 100] /Group << /S /Transparency /CS {space} >>"),
            &format!("{paint} 0 0 50 100 re f"),
        ),
    ];
    let (rgba, width) = render(pdf(&objects));
    let value = |x| {
        let [r, g, b] = pixel(&rgba, width, x, 50);
        // Red over grey: red rises from the backdrop to 255 and green and blue fall to 0 as the
        // mask goes from 0 to 1.
        let from_red = (f64::from(r) - BACKDROP) / (255.0 - BACKDROP);
        let from_green = (BACKDROP - f64::from(g)) / BACKDROP;
        let from_blue = (BACKDROP - f64::from(b)) / BACKDROP;
        assert!(
            (from_red - from_green).abs() < 0.03 && (from_green - from_blue).abs() < 0.03,
            "{space} {paint}: pixel {:?} is not red over the backdrop",
            [r, g, b]
        );
        from_green
    };
    (value(25), value(75))
}

fn assert_mask(space: &str, paint: &str, bc: Option<&str>, left: f64, right: f64) {
    let (got_left, got_right) = mask_values(space, paint, bc);
    assert!((got_left - left).abs() < 0.02, "{space} group, `{paint}`: mask {got_left:.3}, expected {left}");
    assert!((got_right - right).abs() < 0.02, "{space} group, /BC {bc:?}: mask {got_right:.3}, expected {right}");
}

#[test]
fn black_ink_in_a_device_cmyk_group_masks_fully_and_no_ink_shows_fully() {
    assert_mask("/DeviceCMYK", "0 0 0 1 k", None, 0.0, 0.0);
    assert_mask("/DeviceCMYK", "0 0 0 0 k", None, 1.0, 0.0);
    assert_mask("/DeviceCMYK", "0 0 0 0.5 k", None, 0.5, 0.0);
}

#[test]
fn each_process_ink_in_a_device_cmyk_group_masks_by_its_weight() {
    assert_mask("/DeviceCMYK", "1 0 0 0 k", None, 0.7, 0.0);
    assert_mask("/DeviceCMYK", "0 1 0 0 k", None, 0.41, 0.0);
    assert_mask("/DeviceCMYK", "0 0 1 0 k", None, 0.89, 0.0);
    assert_mask("/DeviceCMYK", "1 1 1 1 k", None, 0.0, 0.0);
    // The stroking colour too, and a colour selected with `cs`/`sc`.
    assert_mask("/DeviceCMYK", "0 0 0 1 K 0 0 0 1 k 50 w 25 0 m 25 100 l S", None, 0.0, 0.0);
    assert_mask("/DeviceCMYK", "/DeviceCMYK cs 0 0 0 0.5 sc", None, 0.5, 0.0);
}

#[test]
fn a_device_cmyk_backdrop_colour_masks_by_its_gray() {
    assert_mask("/DeviceCMYK", "0 0 0 1 k", Some("[0 0 0 0]"), 0.0, 1.0);
    assert_mask("/DeviceCMYK", "0 0 0 0 k", Some("[0 0 0 1]"), 1.0, 0.0);
    assert_mask("/DeviceCMYK", "0 0 0 1 k", Some("[1 0 0 0]"), 0.0, 0.7);
}

#[test]
fn gray_and_rgb_groups_mask_as_before() {
    assert_mask("/DeviceGray", "0 g", None, 0.0, 0.0);
    assert_mask("/DeviceGray", "1 g", Some("[1]"), 1.0, 1.0);
    assert_mask("/DeviceGray", "0.5 g", None, 0.5, 0.0);
    assert_mask("/DeviceRGB", "0 0 0 rg", None, 0.0, 0.0);
    assert_mask("/DeviceRGB", "1 1 1 rg", Some("[1 1 1]"), 1.0, 1.0);
    assert_mask("/DeviceRGB", "0.5 0.5 0.5 rg", None, 0.5, 0.0);
}

#[test]
fn device_cmyk_on_the_page_keeps_its_display_colour() {
    // Only a luminosity mask in a DeviceCMYK group reads CMYK as its gray. On the page, CMYK is
    // displayed through the CMYK profile as before, where 100% K is a dark grey.
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R >>".to_string(),
        stream("", "0 0 0 1 k 0 0 100 100 re f"),
    ];
    let (rgba, width) = render(pdf(&objects));
    let [r, g, b] = pixel(&rgba, width, 50, 50);
    assert!(r > 10 && g > 10 && b > 10 && r < 80, "100% K on the page: {:?}", [r, g, b]);
}
