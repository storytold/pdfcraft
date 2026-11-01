//! A page whose group's colour space is `DeviceCMYK` blends in CMYK on the page itself, in the
//! non-isolated and knockout groups that inherit that space, and in isolated groups of its own
//! (ISO 32000-2 §11.3.4, §11.4.7; #4887).

use crate::{PageRenderer, RenderConfig, RenderRequest, RenderedPage, RequestKind};
use std::sync::Arc;

/// The page of #4887's fixture: four 200 pt cells, each a magenta square with black over it in
/// `/BM /Difference`, and a strip of the green that blend gives in CMYK across the top.
///
/// 1. on the page itself;
/// 2. in a non-isolated group;
/// 3. in an isolated group holding its own magenta;
/// 4. in a non-isolated knockout group, as two overlapping black squares (each blends with the
///    group's initial backdrop only, so the overlap is green too).
///
/// `page_group` is the page's `/Group` entries; `extra` is drawn on the page after the cells.
fn fixture(page_group: &str, blend: &str, extra: &str) -> Vec<u8> {
    let cell = |x: u32, y: u32, body: &str| format!("q 1 0 0 1 {x} {y} cm 0 1 0 0 k 0 0 200 200 re f {body} Q\n");
    let mut page = String::new();
    page += &cell(20, 220, "/GSd gs 0 0 0 1 k 40 40 120 120 re f");
    page += &cell(220, 220, "/Fn Do");
    page += &cell(20, 20, "/Fi Do");
    page += &cell(220, 20, "/Fk Do");
    page += "1 0 1 0 k 0 440 440 20 re f\n";
    page += extra;
    let forms = [
        ("/S /Transparency /CS /DeviceCMYK /I false /K false", "/GSd gs 0 0 0 1 k 40 40 120 120 re f".to_string()),
        ("/S /Transparency /CS /DeviceCMYK /I true /K false", "0 1 0 0 k 0 0 200 200 re f /GSd gs 0 0 0 1 k 40 40 120 120 re f".to_string()),
        ("/S /Transparency /CS /DeviceCMYK /I false /K true", "/GSd gs 0 0 0 1 k 20 20 110 110 re f 70 70 110 110 re f".to_string()),
    ];
    let mut objects = String::new();
    for (i, (group, body)) in forms.iter().enumerate() {
        objects += &format!(
            "{} 0 obj << /Type /XObject /Subtype /Form /BBox [0 0 200 200] /Group << {group} >> /Resources << /ExtGState << /GSd 5 0 R >> >> /Length {} >> stream\n{body}\nendstream endobj\n",
            10 + i,
            body.len()
        );
    }
    format!(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 440 460] /Group << /S /Transparency {page_group} >> /Resources << /ExtGState << /GSd 5 0 R >> /XObject << /Fn 10 0 R /Fi 11 0 R /Fk 12 0 R >> >> /Contents 4 0 R >> endobj
4 0 obj << /Length {} >> stream
{page}
endstream endobj
5 0 obj << /Type /ExtGState /BM /{blend} >> endobj
{objects}trailer << /Root 1 0 R >>
%%EOF",
        page.len()
    )
    .into_bytes()
}

fn render(pdf: Vec<u8>) -> RenderedPage {
    let mut r = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
    let p = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, ..Default::default() });
    assert!(p.error.is_none(), "{:?}", p.error);
    p
}

/// The RGB at user-space point (`x`, `y`) of a page rendered at scale 1.
fn px(p: &RenderedPage, x: u32, y: u32) -> Vec<u8> {
    let row = p.height - 1 - y;
    p.rgba[((row * p.width + x) * 4) as usize..][..3].to_vec()
}

fn distance(a: &[u8], b: &[u8]) -> u8 {
    a.iter().zip(b).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0)
}

/// The middle of each cell's black square, and a point of the knockout cell's black squares
/// outside their overlap.
const POINTS: [(&str, u32, u32); 5] = [
    ("1. on the page", 120, 320),
    ("2. non-isolated group", 320, 320),
    ("3. isolated group", 120, 120),
    ("4. knockout group, overlap", 320, 120),
    ("4. knockout group, one square", 250, 50),
];

/// Every cell of #4887's fixture shows green where black meets magenta in Difference, as it
/// does in CMYK: on the page, in a non-isolated group (which blends in its parent's space,
/// Table 145), in an isolated group of its own and in a knockout group. With the page group
/// blending in DeviceRGB the page itself stays magenta.
#[test]
fn a_device_cmyk_page_group_blends_its_page_and_inherited_groups_in_cmyk() {
    let p = render(fixture("/CS /DeviceCMYK /I false /K false", "Difference", ""));
    let green = px(&p, 220, 450);
    assert!(green[1] > 100 && green[0] < 100, "the strip is green: {green:?}");
    for (name, x, y) in POINTS {
        let at = px(&p, x, y);
        assert!(distance(&at, &green) <= 3, "{name}: {at:?}, green {green:?}");
    }

    let rgb = render(fixture("/CS /DeviceRGB /I false /K false", "Difference", ""));
    let on_page = px(&rgb, 120, 320);
    assert!(distance(&on_page, &green) > 100, "a DeviceRGB page group blends in RGB: {on_page:?}");
}

/// A CMYK page group whose contents have no ink tints (here an RGB fill) is drawn exactly as
/// before, the same as a page group blending in DeviceRGB; so is one that blends nothing (every
/// blend mode Normal), which never takes the CMYK path at all.
#[test]
fn a_device_cmyk_page_group_falls_back_to_rgb_when_it_must_or_needs_not_blend() {
    let rgb_fill = "1 0 0 rg 0 0 10 10 re f\n";
    let cmyk = render(fixture("/CS /DeviceCMYK", "Difference", rgb_fill));
    let rgb = render(fixture("/CS /DeviceRGB", "Difference", rgb_fill));
    assert_eq!(cmyk.rgba, rgb.rgba, "contents without tints are drawn as before");

    let cmyk = render(fixture("/CS /DeviceCMYK", "Normal", ""));
    let rgb = render(fixture("/CS /DeviceRGB", "Normal", ""));
    assert_eq!(cmyk.rgba, rgb.rgba, "a page that blends nothing is drawn as before");
}

/// Whether the page of a one-page document with page `group` and `resources` (and the objects
/// `extra`, numbered from 4) is offered to the renderer to blend in CMYK (vendored
/// hayro-interpret patch (71)).
fn offered(group: &str, resources: &str, extra: &[&str]) -> bool {
    use hayro::hayro_interpret::hayro_syntax::Pdf;
    let mut body = format!(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] {group} /Resources << {resources} >> >> endobj
"
    );
    for (i, object) in extra.iter().enumerate() {
        body += &format!("{} 0 obj {object} endobj\n", 4 + i);
    }
    body += "trailer << /Root 1 0 R >>\n%%EOF";
    let pdf = Pdf::new(body.into_bytes()).expect("synthetic pdf");
    let pages = pdf.pages();
    hayro::hayro_interpret::page_blends_in_device_cmyk(pages.first().expect("one page"))
}

const CMYK: &str = "/Group << /S /Transparency /CS /DeviceCMYK >>";
const FORM: &str =
    "<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] /Resources << /ExtGState << /B << /BM /Multiply >> >> >> /Length 0 >> stream\n\nendstream";

/// Every press-ready page has a CMYK page group, so only one that can blend is drawn in CMYK
/// passes: a blend mode other than Normal is in reach in the page's own graphics states (by
/// name or in an array), or in those of a form, a tiling pattern, a shading pattern or a Type 3
/// font it uses.
#[test]
fn a_cmyk_page_group_is_drawn_in_passes_only_when_a_blend_mode_is_in_reach() {
    assert!(offered(CMYK, "/ExtGState << /G << /BM /Difference >> >>", &[]));
    assert!(offered(CMYK, "/ExtGState << /G << /BM [/Compatible /Screen] >> >>", &[]));
    assert!(offered(CMYK, "/XObject << /X 4 0 R >>", &[FORM]));
    let tiling = FORM.replace("/Type /XObject /Subtype /Form", "/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /XStep 10 /YStep 10");
    assert!(offered(CMYK, "/Pattern << /P 4 0 R >>", &[&tiling]));
    assert!(offered(CMYK, "/Pattern << /P << /PatternType 2 /ExtGState << /BM /Overlay >> >> >>", &[]));
    assert!(offered(CMYK, "/Font << /F << /Type /Font /Subtype /Type3 /Resources << /ExtGState << /B << /BM /Darken >> >> >> >> >>", &[]));

    // A page that blends nothing, a page group in another space, and a page with no group.
    assert!(!offered(CMYK, "/ExtGState << /G << /BM /Normal /ca 0.5 >> /H << /BM [/Compatible] >> >>", &[]));
    assert!(!offered("/Group << /S /Transparency /CS /DeviceRGB >>", "/ExtGState << /G << /BM /Difference >> >>", &[]));
    assert!(!offered("", "/ExtGState << /G << /BM /Difference >> >>", &[]));
    // Only a Type 3 font's resources are drawn with.
    assert!(!offered(CMYK, "/Font << /F << /Type /Font /Subtype /Type1 /Resources << /ExtGState << /B << /BM /Darken >> >> >> >> >>", &[]));
    // A blend mode two forms deep is found; a form naming itself in its resources, with
    // nothing that blends, ends the walk.
    let outer = "<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] /Resources << /XObject << /Y 5 0 R >> >> /Length 0 >> stream\n\nendstream";
    assert!(offered(CMYK, "/XObject << /X 4 0 R >>", &[outer, FORM]));
    let cyclic = "<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] /Resources << /XObject << /X 4 0 R >> >> /Length 0 >> stream\n\nendstream";
    assert!(!offered(CMYK, "/XObject << /X 4 0 R >>", &[cyclic]));
}
