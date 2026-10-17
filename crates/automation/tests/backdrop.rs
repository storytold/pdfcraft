//! Add Background on pages that paint their own backdrop, end to end through the tools: what
//! `page_render` draws, `doc_backdrops`, save and reopen, and Remove putting the page back.

use std::path::PathBuf;

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

/// Two 200 × 100 pt pages saying "Hi": page 1 on a white fill of its own (as browsers write),
/// page 2 on bare paper; a third page is a "scan" (a page-sized grey picture).
fn fixture() -> Vec<u8> {
    let text = "0 g BT /F1 24 Tf 20 40 Td (Hi) Tj ET";
    let contents = [format!("q 1 1 1 rg 0 0 200 100 re f Q {text}"), text.to_string(), format!("q 200 0 0 100 0 0 cm /Im0 Do Q {text}")];
    let mut objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [4 0 R 6 0 R 8 0 R] /Count 3 /MediaBox [0 0 200 100] >>".into(),
        "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 1 >>\nstream\n\u{80}\nendstream"
            .into(),
    ];
    for (i, c) in contents.iter().enumerate() {
        objs.push(format!(
            "<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> /XObject << /Im0 3 0 R >> >> >>",
            5 + 2 * i
        ));
        objs.push(format!("<< /Length {} >>\nstream\n{c}\nendstream", c.len()));
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn workdir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pdfcraft-automation-backdrop-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.pdf"), fixture()).unwrap();
    dir
}

fn ok(a: &mut Automation, tool: &str, args: Value) -> Value {
    match a.call(tool, &args) {
        Ok(mut c) => match c.remove(0) {
            Content::Json(v) => v,
            other => panic!("{tool}: expected JSON, got {other:?}"),
        },
        Err(e) => panic!("{tool} {args}: {e}"),
    }
}

/// The RGB of a blank spot (top-right corner area) of a page rendered at 72 dpi.
fn blank_spot(a: &mut Automation, doc: u64, page: u64) -> [u8; 3] {
    let rendered = a.call("page_render", &json!({ "doc": doc, "page": page, "dpi": 72 })).unwrap();
    let Content::Png { data, width, .. } = &rendered[0] else { panic!("expected a PNG") };
    let mut reader = png::Decoder::new(std::io::Cursor::new(data)).read_info().unwrap();
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut pixels).unwrap();
    let channels = info.color_type.samples();
    let at = (10 * *width as usize + 180) * channels;
    [pixels[at], pixels[at + 1], pixels[at + 2]]
}

const RED: [u8; 3] = [255, 0, 0];
const WHITE: [u8; 3] = [255, 255, 255];

#[test]
fn a_background_shows_on_a_page_that_paints_its_own_white_backdrop() {
    let dir = workdir();
    let mut a = Automation::new().with_root(&dir).unwrap();
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    assert_eq!(
        ok(&mut a, "doc_backdrops", json!({ "doc": doc }))["pages"],
        json!([{ "page": 1, "backdrop": "fill" }, { "page": 3, "backdrop": "image" }])
    );

    // Not replacing: the page's own white hides the background, as it always did.
    ok(&mut a, "doc_background", json!({ "doc": doc, "color": "#FF0000", "replace_backdrop": false }));
    assert_eq!(blank_spot(&mut a, doc, 1), WHITE);
    assert_eq!(blank_spot(&mut a, doc, 2), RED);
    ok(&mut a, "edit_undo", json!({ "doc": doc }));

    // The default replaces it: red on both pages; the scan still hides it.
    ok(&mut a, "doc_background", json!({ "doc": doc, "color": "#FF0000" }));
    assert_eq!(blank_spot(&mut a, doc, 1), RED);
    assert_eq!(blank_spot(&mut a, doc, 2), RED);
    assert_ne!(blank_spot(&mut a, doc, 3), RED);
    assert_eq!(ok(&mut a, "doc_backdrops", json!({ "doc": doc }))["pages"], json!([{ "page": 3, "backdrop": "image" }]));

    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "out.pdf" }));
    let saved = ok(&mut a, "doc_open", json!({ "path": "out.pdf" }))["doc"].as_u64().unwrap();
    assert_eq!(blank_spot(&mut a, saved, 1), RED, "after save and reopen");
    ok(&mut a, "doc_remove_marks", json!({ "doc": saved, "kind": "background" }));
    assert_eq!(blank_spot(&mut a, saved, 1), WHITE);
    assert_eq!(
        ok(&mut a, "doc_backdrops", json!({ "doc": saved, "pages": [1] }))["pages"],
        json!([{ "page": 1, "backdrop": "fill" }]),
        "the backdrop is back"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
