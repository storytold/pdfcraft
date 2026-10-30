//! Nonfatal diagnostics on MCP image resources, independently of tool output.

use super::{auto, rpc, workdir};
use base64::Engine as _;
use pdfcraft_automation::mcp::McpServer;
use serde_json::{Value, json};

const WARNING: &str = "a colour that could not be computed was painted in a fallback colour";

fn devicen_pdf(tint_program: &[u8]) -> Vec<u8> {
    fn stream(data: &[u8], extra: &[u8]) -> Vec<u8> {
        let mut object = format!("<< /Length {} ", data.len()).into_bytes();
        object.extend_from_slice(extra);
        object.extend_from_slice(b" >>\nstream\n");
        object.extend_from_slice(data);
        object.extend_from_slice(b"\nendstream");
        object
    }

    let content = b"1 0 0 rg 0 0 100 200 re f /CS1 cs .5 .5 scn 100 0 100 200 re f 0 0 1 rg 0 0 50 50 re f";
    let objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /ColorSpace << /CS1 [/DeviceN [/Spot1 /Spot2] /DeviceRGB 5 0 R] >> >> /Contents 4 0 R >>".to_vec(),
        stream(content, b""),
        stream(tint_program, b"/FunctionType 4 /Domain [0 1 0 1] /Range [0 1 0 1 0 1]"),
    ];

    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        pdf.extend_from_slice(object);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    pdf
}

fn warning_pdf() -> Vec<u8> {
    devicen_pdf(b"{ pop pop pop }")
}

fn clean_pdf() -> Vec<u8> {
    devicen_pdf(b"{ pop pop 0 1 0 }")
}

fn decode_rgba(png_bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let mut reader = png::Decoder::new(std::io::Cursor::new(png_bytes)).read_info().unwrap();
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut pixels).unwrap();
    let pixels = match info.color_type {
        png::ColorType::Rgba => pixels,
        png::ColorType::Rgb => pixels.as_chunks::<3>().0.iter().flat_map(|rgb| [rgb[0], rgb[1], rgb[2], 255]).collect(),
        other => panic!("unexpected PNG colour type {other:?}"),
    };
    (info.width, info.height, pixels)
}

fn pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let offset = ((y * width + x) * 4) as usize;
    pixels[offset..offset + 4].try_into().unwrap()
}

fn resource(server: &mut McpServer, id: u64, uri: &str) -> Value {
    rpc(server, id, "resources/read", json!({ "uri": uri }))
}

fn assert_warning_companion(response: &Value, uri: &str) {
    let contents = response["result"]["contents"].as_array().unwrap();
    assert_eq!(contents.len(), 2, "warning resource must have PNG and JSON parts");
    assert_eq!(contents[0]["uri"], uri);
    assert_eq!(contents[0]["mimeType"], "image/png");
    assert!(contents[0]["blob"].as_str().is_some());
    assert_eq!(contents[1]["uri"], uri, "the diagnostic uses the identical resource URI");
    assert_eq!(contents[1]["mimeType"], "application/json");
    let diagnostic: Value = serde_json::from_str(contents[1]["text"].as_str().unwrap()).unwrap();
    assert_eq!(diagnostic, json!({ "page": 1, "warnings": [WARNING] }));
}

#[test]
fn image_resource_returns_warning_companion_and_repeat_reads_are_stable() {
    let dir = workdir("mcp-resource-warnings");
    let warning_source = warning_pdf();
    let clean_source = clean_pdf();
    std::fs::write(dir.join("fallback.pdf"), &warning_source).unwrap();
    std::fs::write(dir.join("clean.pdf"), &clean_source).unwrap();
    let mut server = McpServer::new(auto(&dir));

    let fallback_open = rpc(
        &mut server,
        1,
        "tools/call",
        json!({
            "name": "doc_open", "arguments": { "path": "fallback.pdf" }
        }),
    );
    let fallback_doc = fallback_open["result"]["structuredContent"]["doc"].as_u64().unwrap();
    let fallback_uri = format!("pdfcraft://doc/{fallback_doc}/page/1/image?dpi=72");
    let page_render = rpc(
        &mut server,
        2,
        "tools/call",
        json!({
            "name": "page_render", "arguments": { "doc": fallback_doc, "page": 1, "dpi": 72 }
        }),
    );
    assert_eq!(page_render["result"]["content"].as_array().unwrap().len(), 1);
    assert_eq!(page_render["result"]["content"][0]["type"], "image");
    assert!(page_render["result"].get("structuredContent").is_none(), "page_render retains its existing response shape");
    let before = rpc(
        &mut server,
        3,
        "tools/call",
        json!({
            "name": "doc_info", "arguments": { "doc": fallback_doc }
        }),
    )["result"]["structuredContent"]
        .clone();

    let first = resource(&mut server, 4, &fallback_uri);
    assert_warning_companion(&first, &fallback_uri);
    let second = resource(&mut server, 5, &fallback_uri);
    assert_warning_companion(&second, &fallback_uri);
    assert!(first["result"]["contents"] == second["result"]["contents"], "repeat reads preserve image and diagnostics");

    let fallback_png = base64::engine::general_purpose::STANDARD.decode(first["result"]["contents"][0]["blob"].as_str().unwrap()).unwrap();
    let (width, height, fallback_pixels) = decode_rgba(&fallback_png);
    assert_eq!((width, height), (200, 200));
    assert_eq!(pixel(&fallback_pixels, width, 75, 50), [255, 0, 0, 255]);
    assert_eq!(pixel(&fallback_pixels, width, 150, 50), [0, 0, 0, 255]);
    assert_eq!(pixel(&fallback_pixels, width, 25, 175), [0, 0, 255, 255]);

    let clean_open = rpc(
        &mut server,
        6,
        "tools/call",
        json!({
            "name": "doc_open", "arguments": { "path": "clean.pdf" }
        }),
    );
    let clean_doc = clean_open["result"]["structuredContent"]["doc"].as_u64().unwrap();
    let clean_uri = format!("pdfcraft://doc/{clean_doc}/page/1/image?dpi=72");
    let clean_page_render = rpc(
        &mut server,
        7,
        "tools/call",
        json!({
            "name": "page_render", "arguments": { "doc": clean_doc, "page": 1, "dpi": 72 }
        }),
    );
    assert_eq!(clean_page_render["result"]["content"].as_array().unwrap().len(), 1);
    assert_eq!(clean_page_render["result"]["content"][0]["type"], "image");
    assert!(clean_page_render["result"].get("structuredContent").is_none());
    let clean_first = resource(&mut server, 8, &clean_uri);
    let clean_contents = clean_first["result"]["contents"].as_array().unwrap();
    assert_eq!(clean_contents.len(), 1, "clean image resource keeps the legacy one-part shape");
    assert_eq!(clean_contents[0]["mimeType"], "image/png");
    assert_eq!(clean_contents[0]["uri"], clean_uri);
    let clean_png = base64::engine::general_purpose::STANDARD.decode(clean_contents[0]["blob"].as_str().unwrap()).unwrap();
    let (clean_width, clean_height, clean_pixels) = decode_rgba(&clean_png);
    assert_eq!((clean_width, clean_height), (200, 200));
    assert_eq!(pixel(&clean_pixels, clean_width, 75, 50), [255, 0, 0, 255]);
    assert_eq!(pixel(&clean_pixels, clean_width, 150, 50), [0, 255, 0, 255]);
    assert_eq!(pixel(&clean_pixels, clean_width, 25, 175), [0, 0, 255, 255]);
    let clean_repeat = resource(&mut server, 9, &clean_uri);
    assert!(clean_repeat["result"]["contents"] == clean_first["result"]["contents"], "clean repeat stays identical");

    let after = rpc(
        &mut server,
        10,
        "tools/call",
        json!({
            "name": "doc_info", "arguments": { "doc": fallback_doc }
        }),
    )["result"]["structuredContent"]
        .clone();
    assert_eq!(before, after, "reading a resource does not mutate document state");
    assert_eq!(std::fs::read(dir.join("fallback.pdf")).unwrap(), warning_source);
    assert_eq!(std::fs::read(dir.join("clean.pdf")).unwrap(), clean_source);
}

#[test]
fn resource_warning_survives_incremental_and_full_save_reopen() {
    let dir = workdir("mcp-resource-warning-save");
    let source = warning_pdf();
    std::fs::write(dir.join("fallback.pdf"), &source).unwrap();
    let mut server = McpServer::new(auto(&dir));
    let opened = rpc(
        &mut server,
        1,
        "tools/call",
        json!({
            "name": "doc_open", "arguments": { "path": "fallback.pdf" }
        }),
    );
    let doc = opened["result"]["structuredContent"]["doc"].as_u64().unwrap();
    let original_uri = format!("pdfcraft://doc/{doc}/page/1/image?dpi=72");
    assert_warning_companion(&resource(&mut server, 2, &original_uri), &original_uri);

    for (full, path) in [(false, "incremental.pdf"), (true, "full.pdf")] {
        let saved = rpc(
            &mut server,
            3,
            "tools/call",
            json!({
                "name": "doc_save", "arguments": { "doc": doc, "path": path, "full": full }
            }),
        );
        assert_eq!(saved["result"]["structuredContent"]["incremental"], !full);
        let reopened = rpc(
            &mut server,
            4,
            "tools/call",
            json!({
                "name": "doc_open", "arguments": { "path": path }
            }),
        );
        let reopened_doc = reopened["result"]["structuredContent"]["doc"].as_u64().unwrap();
        let uri = format!("pdfcraft://doc/{reopened_doc}/page/1/image?dpi=72");
        assert_warning_companion(&resource(&mut server, 5, &uri), &uri);
    }

    assert_eq!(std::fs::read(dir.join("fallback.pdf")).unwrap(), source, "resource reads and saves leave the original source unchanged");
}

#[test]
fn image_resource_keeps_invalid_requests_as_json_rpc_errors() {
    let dir = workdir("mcp-resource-warning-errors");
    let mut server = McpServer::new(auto(&dir));
    let opened = rpc(
        &mut server,
        1,
        "tools/call",
        json!({
            "name": "doc_open", "arguments": { "path": "a.pdf" }
        }),
    );
    let doc = opened["result"]["structuredContent"]["doc"].as_u64().unwrap();

    for (id, uri) in [
        (2, "pdfcraft://doc/999/page/1/image".to_owned()),
        (3, format!("pdfcraft://doc/{doc}/page/0/image")),
        (4, format!("pdfcraft://doc/{doc}/page/4/image")),
        (5, format!("pdfcraft://doc/{doc}/page/1/image?dpi=0")),
        (6, format!("pdfcraft://doc/{doc}/page/1/image?dpi=601")),
        (7, "pdfcraft://unknown".to_owned()),
    ] {
        let response = resource(&mut server, id, &uri);
        assert_eq!(response["error"]["code"], -32602, "{uri}: {response}");
        assert!(response.get("result").is_none(), "invalid resource must not return a partial PNG");
    }
}
