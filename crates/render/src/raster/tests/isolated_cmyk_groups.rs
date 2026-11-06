//! Isolated transparency groups whose colour space is `DeviceCMYK` blend in CMYK (ISO 32000-2
//! §11.3.4, §11.3.5.3).

use crate::{PageRenderer, RenderConfig, RenderRequest, RenderedPage, RequestKind};
use std::sync::Arc;

/// A print test file's patch of blend modes in isolated groups whose colour space is
/// `DeviceCMYK` drew an X in nearly every cell. The elements of a group blend in the
/// group's colour space, and a subtractive space's components are complemented before the
/// blend function and back after it (ISO 32000-2 §11.3.4); of the nonseparable modes,
/// §11.3.5.3 blends C, M and Y as complemented R, G and B and takes K from the backdrop
/// (from the source for Luminosity). PdfCraft blended the colours converted to RGB, so
/// magenta under black with Difference came out magenta instead of green. Vendored
/// hayro-interpret patch (70) and hayro patch (27): an isolated group (§11.4.7: its own
/// transparent backdrop) blending in DeviceCMYK is blended in CMYK.
///
/// Each cell is a square painted in the CMYK the blend should give, with a smaller isolated
/// group on it whose two elements are tint A and then tint B with the cell's blend mode:
/// drawn right, the group is the colour of the square around it.
#[test]
fn isolated_groups_blend_in_device_cmyk() {
    // (blend mode, tint A, tint B, A then B blended in DeviceCMYK, worked out by hand)
    type Cell = (&'static str, [f32; 4], [f32; 4], [f32; 4]);
    let cells: [Cell; 7] = [
        // |(1,0,1,1) − (1,1,1,0)| = (0,1,0,1), complemented back: green.
        ("Difference", [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0], [1.0, 0.0, 1.0, 0.0]),
        ("Exclusion", [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0], [1.0, 0.0, 1.0, 0.0]),
        ("Multiply", [0.0, 0.0, 1.0, 0.0], [0.0, 0.5, 0.0, 0.0], [0.0, 0.5, 1.0, 0.0]),
        ("Screen", [0.0, 1.0, 1.0, 0.0], [0.5, 0.5, 1.0, 0.0], [0.0, 0.5, 1.0, 0.0]),
        ("Darken", [0.0, 0.0, 1.0, 0.0], [0.0, 0.5, 0.0, 0.0], [0.0, 0.5, 1.0, 0.0]),
        // Hue of (0,1,1) at the saturation and luminosity of (1,0.5,1): (0.355,0.855,0.855);
        // K is the backdrop's.
        ("Hue", [0.0, 0.5, 0.0, 0.3], [1.0, 0.0, 0.0, 0.0], [0.645, 0.145, 0.145, 0.3]),
        // (1,0,1) at the luminosity of (0,1,0), clipped: (1,0.3051,1); K is the source's.
        ("Luminosity", [0.0, 1.0, 0.0, 0.3], [1.0, 0.0, 1.0, 0.0], [0.0, 0.6949, 0.0, 0.0]),
    ];
    let tint = |t: [f32; 4]| format!("{} {} {} {} k", t[0], t[1], t[2], t[3]);
    // `group`: the cell group's /Group entries; `b`: what the second element paints.
    let render = |group: &str, b: &dyn Fn([f32; 4]) -> String| {
        let (mut page, mut objects, mut names) = (String::new(), String::new(), String::new());
        for (i, (mode, ta, tb, expected)) in cells.iter().enumerate() {
            let x = 10 + i * 50;
            page += &format!("{} {x} 10 40 40 re f /G{i} Do\n", tint(*expected));
            let rect = format!("{} 20 20 20 re f", x + 10);
            let n = 10 + i * 3;
            for (k, paint) in [(n, tint(*ta)), (n + 1, b(*tb))] {
                let body = format!("{paint} {rect}");
                objects += &format!(
                    "{k} 0 obj << /Type /XObject /Subtype /Form /BBox [{x} 10 {} 50] /Group << /S /Transparency /CS /DeviceCMYK >> /Length {} >> stream\n{body}\nendstream endobj\n",
                    x + 40,
                    body.len()
                );
            }
            let body = "/E1 Do /BM gs /E2 Do";
            objects += &format!(
                "{} 0 obj << /Type /XObject /Subtype /Form /BBox [{x} 10 {} 50] /Group << /S /Transparency {group} >> /Resources << /XObject << /E1 {n} 0 R /E2 {} 0 R >> /ExtGState << /BM << /BM /{mode} >> >> >> /Length {} >> stream\n{body}\nendstream endobj\n",
                n + 2,
                x + 40,
                n + 1,
                body.len()
            );
            names += &format!("/G{i} {} 0 R ", n + 2);
        }
        let pdf = format!(
            "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 370 60] /Group << /S /Transparency /CS /DeviceCMYK >> /Contents 4 0 R /Resources << /XObject << {names}>> >> >> endobj
4 0 obj << /Length {} >> stream
{page}
endstream endobj
{objects}trailer << /Root 1 0 R >>
%%EOF",
            page.len()
        );
        let mut r = PageRenderer::new(Arc::new(pdf.into_bytes()), RenderConfig::default());
        let p = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, ..Default::default() });
        assert!(p.error.is_none(), "{group}: {:?}", p.error);
        p
    };
    let px = |p: &RenderedPage, x: usize, y: u32| p.rgba[((y * p.width + x as u32) * 4) as usize..][..3].to_vec();
    let distance = |a: &[u8], b: &[u8]| a.iter().zip(b).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);

    let p = render("/CS /DeviceCMYK /I true", &tint);
    for (i, (mode, ..)) in cells.iter().enumerate() {
        let (group, square) = (px(&p, 30 + i * 50, 30), px(&p, 14 + i * 50, 30));
        // The square is a solid CMYK fill; the group lands within rounding of it.
        assert!(distance(&group, &square) <= 3, "{mode}: group {group:?}, square {square:?}");
    }

    // A group whose contents have no ink tints (here the second element is RGB) is
    // composited in RGB exactly as before: the same as a group blending in DeviceRGB.
    let rgb = |_: [f32; 4]| "1 0 0 rg".to_string();
    let cmyk_group = render("/CS /DeviceCMYK /I true", &rgb);
    let rgb_group = render("/CS /DeviceRGB /I true", &rgb);
    assert_eq!(cmyk_group.rgba, rgb_group.rgba, "contents without tints are drawn as before");
}

/// Two cases from the review of #3777. A group wider than about two of vello_cpu's tiles lost
/// its blend mode in the tiles a paint covered completely, which drew black over magenta at
/// 300 dpi where Difference gives green; each pass now draws inside a layer. And a nested
/// isolated group that names another blending space (here DeviceRGB) blends in that space
/// (ISO 32000-2 §11.3.4), so the enclosing CMYK group is drawn as before; a non-isolated
/// one ignores its /CS and blends in the enclosing group's CMYK (Table 145).
#[test]
fn isolated_cmyk_groups_blend_at_any_size_and_respect_nested_spaces() {
    let render = |inner_cs: Option<&str>, scale: f32| {
        // `inner_cs`: the nested group's /CS and /I entries.
        let group = "0 1 0 0 k 0 0 200 200 re f /Diff gs 0 0 0 1 k 0 0 200 200 re f";
        let (outer_body, extra) = match inner_cs {
            None => (group.to_string(), String::new()),
            Some(cs) => (
                "/In Do".to_string(),
                format!(
                    "6 0 obj << /Type /XObject /Subtype /Form /BBox [0 0 200 200] /Group << /S /Transparency {cs} >> /Resources << /ExtGState << /Diff << /BM /Difference >> >> >> /Length {} >> stream\n{group}\nendstream endobj\n",
                    group.len()
                ),
            ),
        };
        let page = "q 1 0 0 1 10 70 cm /X0 Do Q 1 0 1 0 k 10 10 200 50 re f";
        let pdf = format!(
            "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 220 280] /Contents 4 0 R /Resources << /XObject << /X0 5 0 R >> >> >> endobj
4 0 obj << /Length {} >> stream
{page}
endstream endobj
5 0 obj << /Type /XObject /Subtype /Form /BBox [0 0 200 200] /Group << /S /Transparency /CS /DeviceCMYK /I true >> /Resources << /XObject << /In 6 0 R >> /ExtGState << /Diff << /BM /Difference >> >> >> /Length {} >> stream
{outer_body}
endstream endobj
{extra}trailer << /Root 1 0 R >>
%%EOF",
            page.len(),
            outer_body.len()
        );
        let mut r = PageRenderer::new(Arc::new(pdf.into_bytes()), RenderConfig::default());
        let p = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale, tag: 0, ..Default::default() });
        assert!(p.error.is_none(), "{:?}", p.error);
        p
    };
    let px = |p: &RenderedPage, x: f32, y: f32, scale: f32| {
        let (x, y) = ((x * scale) as u32, (y * scale) as u32);
        p.rgba[((y * p.width + x) * 4) as usize..][..3].to_vec()
    };
    let distance = |a: &[u8], b: &[u8]| a.iter().zip(b).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);

    // 200 pt at scale 4 is 800 px: the centre lies in tiles the paints cover completely.
    for scale in [1.0, 4.0] {
        let p = render(None, scale);
        let strip = px(&p, 110.0, 245.0, scale);
        for (x, y) in [(110.0, 110.0), (15.0, 15.0), (110.0, 205.0)] {
            let group = px(&p, x, y, scale);
            assert!(distance(&group, &strip) <= 3, "scale {scale} at ({x}, {y}): group {group:?}, strip {strip:?}");
        }
    }

    // A nested DeviceRGB group: the whole CMYK group falls back, so the page is drawn exactly
    // as when the enclosing group is not offered to the CMYK path at all.
    let nested_rgb = render(Some("/CS /DeviceRGB /I true"), 1.0);
    let green = px(&nested_rgb, 110.0, 245.0, 1.0);
    assert!(distance(&px(&nested_rgb, 110.0, 110.0, 1.0), &green) > 100, "the nested group blends in RGB, not CMYK");
    // A nested DeviceCMYK group stays in the CMYK path.
    let nested_cmyk = render(Some("/CS /DeviceCMYK /I true"), 1.0);
    assert!(distance(&px(&nested_cmyk, 110.0, 110.0, 1.0), &green) <= 3);
    // A non-isolated nested group's /CS is ignored (ISO 32000-2 Table 145): it blends in the
    // enclosing group's CMYK, even when it names DeviceRGB.
    let non_isolated_rgb = render(Some("/CS /DeviceRGB /I false /K false"), 1.0);
    assert!(distance(&px(&non_isolated_rgb, 110.0, 110.0, 1.0), &green) <= 3, "a non-isolated nested group inherits CMYK");
}

/// An element of a knockout group is first drawn in erase mode, which clears what the group
/// drew under the element's shape. An isolated `DeviceCMYK` group nested in that element must
/// clear too: when the erase pass drew it through the CMYK path, it painted over the red
/// beneath instead of clearing it. The page must match the same element with its nested group
/// blending in `DeviceRGB`, which never takes the CMYK path.
#[test]
fn an_isolated_cmyk_group_inside_a_knockout_element_is_erased_like_any_other() {
    let render = |inner_cs: &str| {
        let knockout = "1 0 0 rg 20 20 160 160 re f /El Do";
        let element = "/Cm Do";
        let inner = "/A gs 0 1 0 0 k 60 60 80 80 re f";
        let pdf = format!(
            "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Resources << /XObject << /Ko 5 0 R >> >> >> endobj
4 0 obj << /Length 7 >> stream
/Ko Do
endstream endobj
5 0 obj << /Type /XObject /Subtype /Form /BBox [0 0 200 200] /Group << /S /Transparency /I true /K true >> /Resources << /XObject << /El 6 0 R >> >> /Length {} >> stream
{knockout}
endstream endobj
6 0 obj << /Type /XObject /Subtype /Form /BBox [0 0 200 200] /Group << /S /Transparency >> /Resources << /XObject << /Cm 7 0 R >> >> /Length {} >> stream
{element}
endstream endobj
7 0 obj << /Type /XObject /Subtype /Form /BBox [0 0 200 200] /Group << /S /Transparency /CS {inner_cs} /I true >> /Resources << /ExtGState << /A << /ca 0.5 >> >> >> /Length {} >> stream
{inner}
endstream endobj
trailer << /Root 1 0 R >>
%%EOF",
            knockout.len(),
            element.len(),
            inner.len()
        );
        let mut r = PageRenderer::new(Arc::new(pdf.into_bytes()), RenderConfig::default());
        let p = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, ..Default::default() });
        assert!(p.error.is_none(), "{inner_cs}: {:?}", p.error);
        // The centre of the inner square, (100, 100), is also the centre of the page.
        p.rgba[((100 * p.width + 100) * 4) as usize..][..3].to_vec()
    };
    let rgb = render("/DeviceRGB");
    let cmyk = render("/DeviceCMYK");
    assert!(rgb[2] > 150, "the RGB control shows half-opacity magenta over white, not red: {rgb:?}");
    assert!(rgb.iter().zip(&cmyk).all(|(a, b)| a.abs_diff(*b) <= 3), "CMYK {cmyk:?}, RGB {rgb:?}");
}

/// A minimal page with `page` as its content, a shading pattern `/P` and an isolated
/// `DeviceCMYK` group `/G` over `bbox` drawing `group`; the RGB of device pixel (`x`, `y`).
fn cmyk_group_pixel(media: [u32; 2], page: &str, bbox: [u32; 4], group: &str, group_resources: &str, (x, y): (u32, u32)) -> Vec<u8> {
    let pdf = format!(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Contents 4 0 R /Resources << /XObject << /G 5 0 R >> /Pattern << /P 6 0 R >> >> >> endobj
4 0 obj << /Length {} >> stream
{page}
endstream endobj
5 0 obj << /Type /XObject /Subtype /Form /BBox [{} {} {} {}] /Group << /S /Transparency /CS /DeviceCMYK /I true >> /Resources << {group_resources} >> /Length {} >> stream
{group}
endstream endobj
6 0 obj << /Type /Pattern /PatternType 2 /Matrix [2 0 0 2 30 40] /Shading << /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 10 0] /Extend [true true] /Function << /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [0 0 1] /N 1 >> >> >> endobj
trailer << /Root 1 0 R >>
%%EOF",
        media[0],
        media[1],
        page.len(),
        bbox[0],
        bbox[1],
        bbox[2],
        bbox[3],
        group.len()
    );
    let mut r = PageRenderer::new(Arc::new(pdf.into_bytes()), RenderConfig::default());
    let p = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, ..Default::default() });
    assert!(p.error.is_none(), "{:?}", p.error);
    p.rgba[((y * p.width + x) * 4) as usize..][..3].to_vec()
}

/// From the review of #5228: a shading pattern painted before the group left its pattern
/// transform on the context, and the finished group was drawn through it, so the cyan half
/// of the group showed magenta. The group must look the same whether or not a pattern was
/// painted first.
#[test]
fn a_pattern_painted_before_an_isolated_cmyk_group_does_not_move_it() {
    let group = "0 1 0 0 k 20 20 30 60 re f 1 0 0 0 k 50 20 30 60 re f";
    let render = |page: &str| cmyk_group_pixel([100, 100], page, [20, 20, 80, 80], group, "", (55, 50));
    let after_pattern = render("/Pattern cs /P scn 0 0 5 5 re f /G Do");
    let alone = render("/G Do");
    assert!(alone[2] > 150 && alone[0] < 100, "the control is cyan: {alone:?}");
    assert!(alone.iter().zip(&after_pattern).all(|(a, b)| a.abs_diff(*b) <= 3), "after a pattern {after_pattern:?}, alone {alone:?}");
}

/// From the review of #5228: the Color blend mode over an isolated group's empty backdrop
/// keeps the source colour (ISO 32000-2 §11.3.5.3 with nothing beneath). The K ink pass
/// draws gray, and vello_cpu's nonseparable blend of a gray darkened some K levels to
/// near black. Every 8-bit K level is checked against a plain fill of the same ink.
#[test]
fn the_color_blend_mode_over_an_empty_cmyk_group_keeps_every_k_level() {
    let mut wrong = Vec::new();
    for level in 0..=255u8 {
        let k = format!("0 0 0 {} k", f32::from(level) / 255.0);
        let page = format!("{k} 0 0 20 20 re f /G Do");
        let group = format!("/B gs {k} 20 0 20 20 re f");
        let plain = cmyk_group_pixel([40, 20], &page, [20, 0, 40, 20], &group, "/ExtGState << /B << /BM /Color >> >>", (10, 10));
        let blended = cmyk_group_pixel([40, 20], &page, [20, 0, 40, 20], &group, "/ExtGState << /B << /BM /Color >> >>", (30, 10));
        if plain.iter().zip(&blended).any(|(a, b)| a.abs_diff(*b) > 3) {
            wrong.push((level, plain, blended));
        }
    }
    assert!(wrong.is_empty(), "K levels drawn wrong (level, plain, blended): {wrong:?}");
}
