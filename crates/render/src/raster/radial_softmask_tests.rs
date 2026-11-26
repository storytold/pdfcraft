use super::*;

fn masked_shading(shading: &str, extra: &str, ink: &str) -> Vec<u8> {
    let content = format!("20.25 20.25 279.5 279.5 re W n /M gs {ink} 0 0 320 320 re f");
    let pdf = format!(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 320 320] /Contents 4 0 R /Resources << /ExtGState << /M 5 0 R >> /Pattern << /P << /PatternType 2 /Shading << /ShadingType 3 /ColorSpace /DeviceRGB /Coords [160 160 0 160 160 180] /Extend [true true] /Function << /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [1 0.5 0] /N 1 >> >> >> >> >> >> endobj
4 0 obj << /Length {} >> stream
{content}
endstream endobj
5 0 obj << /Type /ExtGState /SMask << /S /Luminosity /G 6 0 R >> >> endobj
6 0 obj << /Type /XObject /Subtype /Form /BBox [0 0 320 320] /Group << /S /Transparency /CS /DeviceCMYK >> /Resources << /Shading << /Sh0 {shading} >> >> /Length 7 >> stream
/Sh0 sh
endstream endobj
{extra}
trailer << /Root 1 0 R >>
%%EOF",
        content.len()
    );
    pdf.into_bytes()
}

fn render_mask(bytes: Vec<u8>, request: RenderRequest) -> RenderedPage {
    let mut renderer = PageRenderer::new(Arc::new(bytes), RenderConfig::default());
    let page = renderer.render(request);
    assert!(page.error.is_none(), "{:?}", page.error);
    page
}

#[test]
fn radial_luminosity_mask_fades_when_painted_by_one_shading() {
    let bytes = masked_shading(RADIAL_MASK, "", "/Pattern cs /P scn");

    let page = render_mask(bytes, RenderRequest { scale: 1.0, ..Default::default() });

    for (x, blue) in [(160_u32, 0..=5), (250, 124..=132), (299, 200..=250), (300, 255..=255)] {
        let offset = usize::try_from((160 * page.width + x) * 4).unwrap();
        let pixel = &page.rgba[offset..offset + 4];
        assert!(blue.contains(&pixel[2]), "centre/midpoint/antialiased clip/outside, x {x}: {pixel:?}, blue {blue:?}");
    }
}

const RADIAL_MASK: &str = "<< /ShadingType 3 /ColorSpace /DeviceGray /Coords [160 160 0 160 160 180] /Extend [true true] /AntiAlias false /Function << /FunctionType 2 /Domain [0 1] /C0 [1] /C1 [0] /N 1 >> >>";

#[test]
fn radial_luminosity_mask_matches_per_pixel_shading_when_scaled_or_tiled() {
    let code = "{ 160 sub dup mul exch 160 sub dup mul add sqrt 180 div 1 exch sub }";
    let extra = format!("7 0 obj << /FunctionType 4 /Domain [0 320 0 320] /Range [0 1] /Length {} >> stream\n{code}\nendstream endobj", code.len());
    for ink in ["1 0 0 rg", "/Pattern cs /P scn"] {
        let radial = masked_shading(RADIAL_MASK, "", ink);
        let sampled = masked_shading("<< /ShadingType 1 /ColorSpace /DeviceGray /Domain [0 320 0 320] /Function 7 0 R >>", &extra, ink);
        for scale in [0.5, 1.0, 2.0] {
            for preview in [false, true] {
                for tile in [None, Some(Tile { x: 64, y: 64, w: 160, h: 160 })] {
                    let request = RenderRequest { scale, preview, tile, ..Default::default() };

                    let native = render_mask(radial.clone(), request);
                    let reference = render_mask(sampled.clone(), request);

                    assert_eq!((native.width, native.height), (reference.width, reference.height));
                    let worst = native.rgba.iter().zip(reference.rgba.iter()).map(|(a, b)| a.abs_diff(*b)).max();
                    assert!(worst.is_some_and(|d| d <= 4), "{ink}, {request:?}: radial luminosity mask differs from sampled reference by {worst:?}");
                }
            }
        }
    }
}
