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
    named_workdir("main")
}

/// A folder of its own per test: the tests of this binary run at the same time.
fn named_workdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pdfcraft-automation-backdrop-{}-{name}", std::process::id()));
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

/// One 200 × 100 pt page on a white fill of its own: a line to redact, invisible (`3 Tr`) text
/// that Remove Hidden Information removes, and a line that stays (`0 Tr` again: the render mode
/// outlives `ET`). With `tag`, the content is a
/// backdrop copy carrying that tag, under a background.
fn secret_pdf(tag: Option<&str>) -> Vec<u8> {
    let fill = if tag.is_some() { "n" } else { "f" };
    let content = format!(
        "1 1 1 rg 0 0 200 100 re {fill} 0 g BT /F1 12 Tf 10 70 Td (SECRET-LINE) Tj ET BT 3 Tr /F1 12 Tf 10 45 Td (HIDDEN-LINE) Tj ET BT 0 Tr /F1 12 Tf 10 20 Td (KEEP) Tj ET"
    );
    let bg = "q 1 0 0 rg 0 0 200 100 re f Q";
    let (contents, extra) = match tag {
        Some(t) => (
            "[5 0 R 6 0 R]",
            vec![
                format!("<< /PCMark /Background /Length {} >>\nstream\n{bg}\nendstream", bg.len()),
                format!("<< /PCBackdrop {t} /Length {} >>\nstream\n{content}\nendstream", content.len()),
            ],
        ),
        None => ("[5 0 R]", vec![format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len())]),
    };
    let mut objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 100] >>".into(),
        format!("<< /Type /Page /Parent 2 0 R /Contents {contents} /Resources << /Font << /F1 4 0 R >> >> >>"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
    ];
    objs.extend(extra);
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

/// Every stream of a saved file, decoded, joined: what anyone reading the file can recover.
fn every_stream(path: &std::path::Path) -> String {
    let doc = pdfcraft_cos::Document::open(std::sync::Arc::new(std::fs::read(path).unwrap())).unwrap();
    doc.scan_objects()
        .filter_map(|(_, o)| match &*o {
            pdfcraft_cos::Object::Stream(s) => Some(String::from_utf8_lossy(&s.decoded().unwrap_or_default()).into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Text a redaction or Remove Hidden Information took off a page whose backdrop a background
/// replaced is gone from the saved file. The backdrop copy's tag used to keep a reference to the
/// page's original content stream, and with it the removed text. Removing the background
/// afterwards still turns the page's white fill back on.
#[test]
fn removed_text_leaves_the_file_when_a_background_replaced_the_pages_backdrop() {
    for (remove, gone) in [
        (vec![("redact_mark", json!({ "find": "SECRET-LINE" })), ("redact_apply", json!({}))], "SECRET-LINE"),
        (vec![("doc_remove_hidden", json!({ "categories": ["hidden-text"] }))], "HIDDEN-LINE"),
    ] {
        let dir = named_workdir(gone);
        std::fs::write(dir.join("s.pdf"), secret_pdf(None)).unwrap();
        let mut a = Automation::new().with_root(&dir).unwrap();
        let doc = ok(&mut a, "doc_open", json!({ "path": "s.pdf" }))["doc"].as_u64().unwrap();
        ok(&mut a, "doc_background", json!({ "doc": doc, "color": "#FF0000" }));
        for (tool, args) in &remove {
            let mut args = args.clone();
            args["doc"] = json!(doc);
            ok(&mut a, tool, args);
        }
        ok(&mut a, "doc_save", json!({ "doc": doc, "path": "out.pdf", "full": true }));
        let file = every_stream(&dir.join("out.pdf"));
        assert!(file.contains("(KEEP)"), "{gone}: the page's other text is still there: {file}");
        assert!(!file.contains(gone), "{gone} is still in the file: {file}");

        let saved = ok(&mut a, "doc_open", json!({ "path": "out.pdf" }))["doc"].as_u64().unwrap();
        let removed = ok(&mut a, "doc_remove_marks", json!({ "doc": saved, "kind": "background" }));
        assert!(removed.get("warnings").is_none(), "{gone}: {removed}");
        assert_eq!(
            ok(&mut a, "doc_backdrops", json!({ "doc": saved }))["pages"],
            json!([{ "page": 1, "backdrop": "fill" }]),
            "{gone}: the fill is back"
        );
        assert_eq!(blank_spot(&mut a, saved, 1), WHITE);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// When the backdrop can't be turned back on safely, Remove background says which pages keep it
/// off.
#[test]
fn remove_background_warns_about_pages_whose_own_backdrop_stays_off() {
    let dir = named_workdir("left-off");
    // A tag whose hash doesn't match the bytes before the fill: they changed since.
    let tag = format!("<< /Index 2 /Op (f) /Before <{}> >>", "00".repeat(32));
    std::fs::write(dir.join("t.pdf"), secret_pdf(Some(&tag))).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap();
    let doc = ok(&mut a, "doc_open", json!({ "path": "t.pdf" }))["doc"].as_u64().unwrap();
    let removed = ok(&mut a, "doc_remove_marks", json!({ "doc": doc, "kind": "background" }));
    let warnings = removed["warnings"].as_array().unwrap_or_else(|| panic!("{removed}"));
    assert_eq!(warnings.len(), 1, "{removed}");
    assert!(warnings[0].as_str().unwrap().starts_with("page 1 changed after the background was added"), "{removed}");
    assert_eq!(ok(&mut a, "doc_backdrops", json!({ "doc": doc }))["pages"], json!([]), "the fill stays off");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Two 200 × 100 pt pages whose full-page `1 g` fill is not a plain white backdrop (#2815
/// review): page 1 paints it half transparent, through a `gs` that sets `ca` and a later one
/// that only sets the line width; page 2 sends it through a transfer function that maps 1 to 0.
fn graphics_state_pdf() -> Vec<u8> {
    let invert = "<< /FunctionType 2 /Domain [0 1] /C0 [1] /C1 [0] /N 1 >>";
    let resources = format!("<< /ExtGState << /Transparent << /ca 0.5 >> /LineOnly << /LW 2 >> /Invert << /TR {invert} >> >> >>");
    let contents = ["/Transparent gs /LineOnly gs 1 g 0 0 200 100 re f", "/Invert gs 1 g 0 0 200 100 re f"];
    let mut objs: Vec<String> =
        vec!["<< /Type /Catalog /Pages 2 0 R >>".into(), "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 /MediaBox [0 0 200 100] >>".into()];
    for (i, c) in contents.iter().enumerate() {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources {resources} >>", 4 + 2 * i));
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

/// #2815 review: a translucent fill (its `ca` left in force by a later `gs`) and a fill through
/// a transfer function are not backdrops, so a red background goes under them and the
/// translucent white still lightens it.
#[test]
fn fills_the_graphics_state_changes_are_not_replaced() {
    let dir = named_workdir("graphics-state");
    std::fs::write(dir.join("gs.pdf"), graphics_state_pdf()).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap();
    let doc = ok(&mut a, "doc_open", json!({ "path": "gs.pdf" }))["doc"].as_u64().unwrap();
    assert_eq!(ok(&mut a, "doc_backdrops", json!({ "doc": doc }))["pages"], json!([]), "neither fill is a backdrop");
    ok(&mut a, "doc_background", json!({ "doc": doc, "color": "#FF0000" }));
    let [r, g, b] = blank_spot(&mut a, doc, 1);
    assert_eq!(r, 255, "{:?}", [r, g, b]);
    assert!((100..=160).contains(&g) && (100..=160).contains(&b), "half-transparent white over red, not red: {:?}", [r, g, b]);
    let _ = std::fs::remove_dir_all(&dir);
}
