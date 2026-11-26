//! The nonseparable blend modes keep grays gray (ISO 32000-2 §11.3.5.3). vello_cpu 0.0.8's
//! `clip_color` divided 0 by 0 for grays that rounding left just outside 0..1 and drew them black
//! under `Color`. Upstream's report also has black under `Luminosity` over a gray going wrong; that
//! case already renders correctly here, so its test guards rather than reproduces.

use std::sync::Arc;

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind};

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

/// Renders an isolated group holding a 16 × 16 grid of 6 pt cells, one per gray level, each
/// drawn by `cell(level, "x y 6 6 re")` with `/B` (`/BM /{mode} /ca {alpha}`) available, and
/// returns each level's centre pixel.
fn gray_ramp(mode: &str, alpha: f32, cell: impl Fn(u8, &str) -> String) -> Vec<(u8, [u8; 3])> {
    let mut content = String::new();
    for level in 0..=255_u8 {
        let (x, y) = (2 + u32::from(level % 16) * 6, 2 + u32::from(level / 16) * 6);
        content.push_str(&cell(level, &format!("{x} {y} 6 6 re")));
        content.push('\n');
    }
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R /Resources << /XObject << /Fm0 5 0 R >> >> >>".to_string(),
        stream("", "/Fm0 Do"),
        stream(
            &format!(
                "/Type /XObject /Subtype /Form /BBox [0 0 100 100] /Group << /S /Transparency /I true >> /Resources << /ExtGState << /B << /BM /{mode} /ca {alpha} >> >> >>"
            ),
            &content,
        ),
    ];
    let mut renderer = PageRenderer::new(Arc::new(pdf(&objects)), RenderConfig::default());
    let page = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, preview: false });
    assert!(page.error.is_none(), "{:?}", page.error);
    (0..=255_u8)
        .map(|level| {
            let (x, y) = (5 + u32::from(level % 16) * 6, 100 - 5 - u32::from(level / 16) * 6);
            let offset = usize::try_from((y * page.width + x) * 4).unwrap();
            (level, [page.rgba[offset], page.rgba[offset + 1], page.rgba[offset + 2]])
        })
        .collect()
}

fn gray(level: u8) -> f32 {
    f32::from(level) / 255.0
}

fn assert_ramp(what: &str, got: &[(u8, [u8; 3])], expected: impl Fn(u8) -> f32) {
    let wrong: Vec<_> = got.iter().filter(|(level, rgb)| rgb.iter().any(|&c| (f32::from(c) - expected(*level)).abs() > 2.0)).collect();
    assert!(wrong.is_empty(), "{what}: {} levels off, (level, pixel): {wrong:?}", wrong.len());
}

#[test]
fn every_gray_under_color_over_nothing_keeps_its_lightness() {
    // With nothing beneath it the blend result is the source (§11.4.7): each gray at half
    // opacity, over the white page.
    let got = gray_ramp("Color", 0.5, |level, rect| format!("/B gs {} g {rect} f", gray(level)));
    assert_ramp("Color", &got, |level| (f32::from(level) + 255.0) / 2.0);
}

#[test]
fn black_under_luminosity_halves_every_opaque_gray() {
    // B(Cb, Cs) = SetLum(Cb, Lum(Cs)) is black for a black source, so at half opacity the
    // result is half the backdrop's gray.
    let got = gray_ramp("Luminosity", 0.5, |level, rect| format!("q {} g {rect} f /B gs 0 g {rect} f Q", gray(level)));
    assert_ramp("Luminosity", &got, |level| f32::from(level) / 2.0);
}

#[test]
fn a_gray_into_an_empty_group_keeps_its_lightness_in_every_nonseparable_mode() {
    for mode in ["Color", "Luminosity", "Hue", "Saturation"] {
        let got = gray_ramp(mode, 1.0, |level, rect| format!("/B gs {} g {rect} f", gray(level)));
        assert_ramp(mode, &got, f32::from);
    }
}
