//! Contributor-original synthetic forms for named marked-content properties.
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind};
use std::sync::Arc;

fn fixture(own: bool) -> Vec<u8> {
    let res1 = "<< /Font << /F1 3 0 R >> /XObject << /Fm0 8 0 R >> /Properties << /P1 << /ActualText (REPLACED) >> >> >>";
    let res2 = "<< /Font << /F1 3 0 R >> /XObject << /Fm0 9 0 R >> /Properties << /oc1 7 0 R >> >>";
    let own1 = if own { format!("/Resources {res1}") } else { String::new() };
    let own2 = if own { format!("/Resources {res2}") } else { String::new() };
    let body1 = "/Span /P1 BDC BT /F1 24 Tf 72 700 Td (glyphs) Tj ET EMC";
    let body2 = "/OC /oc1 BDC BT /F1 24 Tf 72 700 Td (HIDDENTEXT) Tj ET EMC";
    let page1 = "q /Fm0 Do Q";
    let page2 = "q /Fm0 Do Q BT /F1 24 Tf 72 600 Td (visible) Tj ET";
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [7 0 R] /D << /OFF [7 0 R] >> >> >>".into(),
        "<< /Type /Pages /Kids [4 0 R 5 0 R] /Count 2 >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 6 0 R /Resources {res1} >>"),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 10 0 R /Resources {res2} >>"),
        stream("", page1),
        "<< /Type /OCG /Name (Hidden) >>".into(),
        stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 612 792] {own1}"), body1),
        stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 612 792] {own2}"), body2),
        stream("", page2),
    ])
}

fn text(renderer: &mut PageRenderer, page: usize) -> Arc<pdfcraft_render::PageText> {
    let result = renderer.render(RenderRequest { page, kind: RequestKind::Text, ..Default::default() });
    assert!(result.error.is_none(), "{:?}", result.error);
    result.text.unwrap()
}

#[test]
fn named_actual_text_in_resource_less_form_replaces_glyphs() {
    for own in [true, false] {
        let mut renderer = PageRenderer::new(Arc::new(fixture(own)), RenderConfig::default());
        let result = text(&mut renderer, 0);
        assert_eq!(result.plain_text(), "REPLACED", "own Resources: {own}");
        assert_eq!(result.find("REPLACED").len(), 1);
        assert!(result.find("glyphs").is_empty());
    }
}

#[test]
fn named_off_layer_in_resource_less_form_stays_hidden() {
    for own in [true, false] {
        let mut renderer = PageRenderer::new(Arc::new(fixture(own)), RenderConfig::default());
        let image = renderer.render(RenderRequest { page: 1, scale: 1.0, ..Default::default() });
        assert!(image.error.is_none(), "{:?}", image.error);
        assert_eq!((image.width, image.height), (612, 792));
        let upper = image
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .filter(|(index, _)| (70..105).contains(&(index / 612)))
            .filter(|(_, pixel)| **pixel != [255, 255, 255, 255])
            .count();
        let lower = image
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .filter(|(index, _)| (170..200).contains(&(index / 612)))
            .filter(|(_, pixel)| **pixel != [255, 255, 255, 255])
            .count();
        let result = text(&mut renderer, 1);
        println!(
            "own={own}: upper ink={upper}; lower ink={lower}; text={:?}; hidden Find={:?}; visible Find={:?}",
            result.plain_text(),
            result.find("HIDDENTEXT"),
            result.find("visible")
        );
        assert_eq!(upper, 0, "OFF layer has no upper ink, own Resources: {own}");
        assert!(lower > 0, "ordinary page text stays visible");
        assert_eq!(result.plain_text(), "visible", "own Resources: {own}");
        assert_eq!(result.find("visible").len(), 1);
        assert!(result.find("HIDDENTEXT").is_empty());
    }
}

// Contributor-original synthetic PDFs. No font or document assets are embedded.
fn pdf(objects: &[String]) -> Vec<u8> {
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    bytes
}

fn stream(dict: &str, body: &str) -> String {
    format!("<< {dict} /Length {} >>\nstream\n{body}\nendstream", body.len())
}

fn resources(properties: &str) -> String {
    format!("<< /Font << /F1 3 0 R >> /XObject << /Fm 6 0 R /Inner 7 0 R >> {properties} >>")
}

fn form(resources: &str, body: &str) -> String {
    stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 200 200] {resources}"), body)
}

const GLYPHS: &str = "/Span /P BDC BT /F1 12 Tf 10 150 Td (glyphs) Tj ET EMC";
const OFF_PAINT: &str = "/OC /O BDC 1 0 0 rg 10 100 20 20 re f EMC 0 0 1 rg 50 100 20 20 re f";

fn objects(page_resources: &str, form_resources: &str, body: &str) -> Vec<String> {
    vec![
        "<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [8 0 R 9 0 R] /D << /OFF [8 0 R] >> >> >>".into(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 5 0 R {page_resources} >>"),
        stream("", "/Fm Do"),
        form(form_resources, body),
        form("", GLYPHS),
        "<< /Type /OCG /Name (PageOff) >>".into(),
        "<< /Type /OCG /Name (FormOn) >>".into(),
    ]
}

fn assert_text(bytes: Vec<u8>, page: usize, expected: &str) {
    let mut renderer = PageRenderer::new(Arc::new(bytes), RenderConfig::default());
    let result = text(&mut renderer, page);
    assert_eq!(result.plain_text(), expected);
    assert_eq!(result.find(expected).len(), 1);
    if expected != "glyphs" {
        assert!(result.find("glyphs").is_empty());
    }
}

#[test]
fn own_form_dictionary_stops_property_fallback() {
    let page = format!("/Resources {}", resources("/Properties << /P << /ActualText (PAGE) >> >>"));
    for (own, expected) in [
        ("", "PAGE"),
        ("/Resources << /Properties << /P << /ActualText (OWN) >> >> >>", "OWN"),
        ("/Resources << >>", "glyphs"),
        ("/Resources << /Properties << >> >>", "glyphs"),
        ("/Resources << /Properties << /Other << /ActualText (OTHER) >> >> >>", "glyphs"),
        ("/Resources << /Properties << /P 42 >> >>", "glyphs"),
        ("/Resources << /Properties << /P 99 0 R >> >>", "glyphs"),
        ("/Resources << /Properties 42 >>", "glyphs"),
        ("/Resources 42", "glyphs"),
        ("/Resources 99 0 R", "glyphs"),
        // Preserve the pre-fix behavior for an explicitly present null entry.
        ("/Resources null", "glyphs"),
    ] {
        assert_text(pdf(&objects(&page, own, GLYPHS)), 0, expected);
    }
}

#[test]
fn nested_resource_less_form_uses_page_origin_and_own_form_shadows_it() {
    let page = format!("/Resources {}", resources("/Properties << /P << /ActualText (PAGE) >> >>"));
    let outer = format!("/Resources {}", resources("/Properties << /P << /ActualText (CALLER) >> >>"));
    for (inner, expected) in [("", "PAGE"), ("/Resources << /Properties << /P << /ActualText (INNER) >> >> >>", "INNER")] {
        let mut fixture = objects(&page, &outer, "/Inner Do");
        fixture[6] = form(inner, GLYPHS);
        assert_text(pdf(&fixture), 0, expected);
    }
}

#[test]
fn shared_form_uses_each_pages_properties() {
    let first = format!("/Resources {}", resources("/Properties << /P << /ActualText (FIRST) >> >>"));
    let second = format!("/Resources {}", resources("/Properties << /P << /ActualText (SECOND) >> >>"));
    let mut fixture = objects(&first, "", GLYPHS);
    fixture[1] = "<< /Type /Pages /Kids [4 0 R 10 0 R] /Count 2 >>".into();
    fixture.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 5 0 R {second} >>"));
    let bytes = pdf(&fixture);
    assert_text(bytes.clone(), 0, "FIRST");
    assert_text(bytes, 1, "SECOND");
}

#[test]
fn page_tree_properties_inherit_only_until_an_own_resources_dictionary() {
    let root = resources("/Properties << /P << /ActualText (ROOT) >> >>");
    let near = resources("/Properties << /P << /ActualText (NEAR) >> >>");
    for (node, page, expected) in [
        ("".to_string(), "".to_string(), "ROOT"),
        (format!("/Resources {near}"), "".to_string(), "NEAR"),
        ("/Resources << >>".to_string(), "".to_string(), "glyphs"),
        ("".to_string(), "/Resources << >>".to_string(), "glyphs"),
    ] {
        let mut fixture = objects(&page, "", GLYPHS);
        fixture[1] = format!("<< /Type /Pages /Resources {root} /Kids [10 0 R] /Count 1 >>");
        fixture[3] = fixture[3].replace("/Parent 2 0 R", "/Parent 10 0 R");
        fixture.push(format!("<< /Type /Pages /Parent 2 0 R /Kids [4 0 R] /Count 1 {node} >>"));
        assert_text(pdf(&fixture), 0, expected);
    }
}

fn assert_paint(bytes: Vec<u8>, page: usize, layers: Vec<(i32, i32, bool)>, visible: bool) {
    let mut renderer = PageRenderer::new(Arc::new(bytes), RenderConfig { layers: Arc::new(layers), ..Default::default() });
    let image = renderer.render(RenderRequest { page, scale: 1.0, ..Default::default() });
    assert!(image.error.is_none(), "{:?}", image.error);
    assert_eq!((image.width, image.height), (200, 200));
    let pixel = |x: usize, y: usize| &image.rgba[(y * 200 + x) * 4..(y * 200 + x) * 4 + 4];
    assert_eq!(pixel(60, 90), [0, 0, 255, 255], "unmarked control remains blue");
    assert_eq!(pixel(20, 90), if visible { [255, 0, 0, 255] } else { [255; 4] });
}

#[test]
fn indirect_page_properties_preserve_ocg_identity_and_visibility_overrides() {
    let page = format!("/Resources {}", resources("/Properties 10 0 R"));
    let mut fixture = objects(&page, "", OFF_PAINT);
    fixture.push("<< /O 8 0 R >>".into());
    let bytes = pdf(&fixture);
    assert_paint(bytes.clone(), 0, vec![], false);
    assert_paint(bytes.clone(), 0, vec![(9, 0, true)], false);
    assert_paint(bytes.clone(), 0, vec![(8, 0, true)], true);
    assert_paint(bytes, 0, vec![(8, 0, false)], false);
}

#[test]
fn nested_ocg_uses_page_origin_and_own_dictionary_boundaries() {
    let page = format!("/Resources {}", resources("/Properties << /O 8 0 R >>"));
    let caller = format!("/Resources {}", resources("/Properties << /O 9 0 R >>"));
    for (inner, visible) in [
        ("", false),
        ("/Resources << /Properties << /O 9 0 R >> >>", true),
        ("/Resources << >>", true),
        ("/Resources << /Properties << /Other 8 0 R >> >>", true),
        ("/Resources << /Properties << /O 42 >> >>", true),
        ("/Resources << /Properties << /O 99 0 R >> >>", true),
    ] {
        let mut fixture = objects(&page, &caller, "/Inner Do");
        fixture[6] = form(inner, OFF_PAINT);
        assert_paint(pdf(&fixture), 0, vec![], visible);
    }
}

#[test]
fn direct_and_inline_properties_and_unmarked_content_keep_existing_behavior() {
    let page = format!("/Resources {}", resources("/Properties << /P << /ActualText (DIRECT) >> >>"));
    for (body, expected) in [
        (GLYPHS, "DIRECT"),
        ("/Span << /ActualText (INLINE) >> BDC BT /F1 12 Tf 10 150 Td (glyphs) Tj ET EMC", "INLINE"),
        ("/Span BMC BT /F1 12 Tf 10 150 Td (glyphs) Tj ET EMC", "glyphs"),
        ("/Span << /ActualText (OUTER) >> BDC /Span /P BDC BT /F1 12 Tf 10 150 Td (glyphs) Tj ET EMC EMC", "OUTER"),
    ] {
        assert_text(pdf(&objects(&page, "", body)), 0, expected);
    }
    let inline_ref = OFF_PAINT.replace("/O BDC", "<< /OC 8 0 R >> BDC");
    assert_paint(pdf(&objects(&page, "", &inline_ref)), 0, vec![], false);
    assert_paint(pdf(&objects(&page, "", &inline_ref)), 0, vec![(8, 0, true)], true);
    let direct_ocg = format!("/Resources {}", resources("/Properties << /O << /Type /OCG /Name (Direct) >> >>"));
    assert_paint(pdf(&objects(&direct_ocg, "", OFF_PAINT)), 0, vec![], true);
    assert_paint(pdf(&objects(&page, "", &OFF_PAINT.replace("/O BDC", "<< /Type /OCG /Name (Inline) >> BDC"))), 0, vec![], true);
}

#[test]
fn save_reopen_preserves_missing_resources_and_effective_named_properties() {
    let page = format!("/Resources {}", resources("/Properties << /P << /ActualText (PAGE) >> /O 8 0 R >>"));
    for own in ["", "/Resources << /Properties << /P << /ActualText (PAGE) >> /O 8 0 R >> >>"] {
        let bytes = pdf(&objects(&page, own, GLYPHS));
        let mut document = lopdf::Document::load_mem(&bytes).unwrap();
        let before = document.get_object((6, 0)).unwrap().as_stream().unwrap().dict.clone();
        let mut saved = Vec::new();
        document.save_to(&mut saved).unwrap();
        let reopened = lopdf::Document::load_mem(&saved).unwrap();
        assert_eq!(reopened.get_object((6, 0)).unwrap().as_stream().unwrap().dict, before);
        assert_text(saved, 0, "PAGE");
        let painted = objects(&page, own, OFF_PAINT);
        let mut document = lopdf::Document::load_mem(&pdf(&painted)).unwrap();
        let mut saved = Vec::new();
        document.save_to(&mut saved).unwrap();
        assert_paint(saved, 0, vec![], false);
    }
}

#[test]
fn shared_form_uses_each_pages_ocg_reference() {
    let first = format!("/Resources {}", resources("/Properties << /O 8 0 R >>"));
    let second = format!("/Resources {}", resources("/Properties << /O 9 0 R >>"));
    let mut fixture = objects(&first, "", OFF_PAINT);
    fixture[1] = "<< /Type /Pages /Kids [4 0 R 10 0 R] /Count 2 >>".into();
    fixture.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 5 0 R {second} >>"));
    let bytes = pdf(&fixture);
    assert_paint(bytes.clone(), 0, vec![], false);
    assert_paint(bytes.clone(), 1, vec![], true);
    assert_paint(bytes.clone(), 0, vec![(8, 0, true)], true);
    assert_paint(bytes, 1, vec![(9, 0, false)], false);
}
