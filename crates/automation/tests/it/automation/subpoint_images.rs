use super::*;

#[test]
fn natural_image_aspect_accepts_subpoint_rectangles() {
    let dir = workdir("natural-subpoint-images");
    let mut a = auto(&dir);
    let pixels = [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0, 255, 0, 255, 0, 255, 255];

    for (x, y) in [(2000, 1), (1, 2000), (2_000_000, 1), (1, 2_000_000)] {
        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, 3, 2);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_pixel_dims(Some(png::PixelDimensions { xppu: x, yppu: y, unit: png::Unit::Unspecified }));
            encoder.write_header().unwrap().write_image_data(&pixels).unwrap();
        }
        let name = format!("ratio-{x}-{y}.png");
        std::fs::write(dir.join(&name), &png).unwrap();
        let doc = ok(&mut a, "doc_create", json!({"from": "blank", "width": 64, "height": 32}))["doc"].as_u64().unwrap();
        let added = ok(&mut a, "page_add_image", json!({"doc": doc, "page": 1, "path": name}));
        let density = f64::from(x.max(y));
        let natural = (3.0 * density / f64::from(x), 2.0 * density / f64::from(y));
        let fit = (51.2 / natural.0).min(25.6 / natural.1).min(1.0);
        let expected = (natural.0 * fit, natural.1 * fit);
        let check_rect = |rect: &serde_json::Value| {
            let rect = rect.as_array().unwrap();
            let width = (rect[2].as_f64().unwrap() - rect[0].as_f64().unwrap()).abs();
            let height = (rect[3].as_f64().unwrap() - rect[1].as_f64().unwrap()).abs();
            assert!(width > 0.0 && height > 0.0);
            assert!((width - expected.0).abs() <= expected.0 * 1e-9, "{x}:{y} width {width} must retain {}", expected.0);
            assert!((height - expected.1).abs() <= expected.1 * 1e-9, "{x}:{y} height {height} must retain {}", expected.1);
        };
        check_rect(&added["rect"]);
        let saved = format!("{name}.pdf");
        ok(&mut a, "doc_save", json!({"doc": doc, "path": saved}));
        ok(&mut a, "doc_close", json!({"doc": doc}));
        let reopened = ok(&mut a, "doc_open", json!({"path": saved}))["doc"].as_u64().unwrap();
        let listed = ok(&mut a, "content_list", json!({"doc": reopened}));
        assert_eq!(listed["count"], 1);
        check_rect(&listed["items"][0]["rect"]);
        let copied = ok(&mut a, "content_copy", json!({"doc": reopened, "page": 1, "index": 1}));
        check_rect(&copied["rect"]);
        ok(&mut a, "doc_save", json!({"doc": reopened, "path": saved}));
        ok(&mut a, "doc_close", json!({"doc": reopened}));
        let reopened = ok(&mut a, "doc_open", json!({"path": saved}))["doc"].as_u64().unwrap();
        let copied_items = ok(&mut a, "content_list", json!({"doc": reopened}));
        assert_eq!(copied_items["count"], 2);
        for item in copied_items["items"].as_array().unwrap() {
            check_rect(&item["rect"]);
        }
        let rendered = a.call("page_render", &json!({"doc": reopened, "page": 1, "dpi": 72})).unwrap();
        let Content::Png { width, height, .. } = &rendered[0] else { panic!("rendered added image") };
        assert_eq!((*width, *height), (64, 32));
        assert_eq!(std::fs::read(dir.join(&name)).unwrap(), png, "Add image must not change its source");
        ok(&mut a, "doc_close", json!({"doc": reopened}));
    }
}
