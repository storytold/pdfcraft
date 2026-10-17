//! `pdfcraft-cli mcp --automation-read-root <dir> --automation-write-root <dir>`: through the MCP
//! protocol, files open and load only from inside the read root and are written only inside the
//! write root. Paths that leave a root (`..`, another folder, a sibling with the same prefix, a
//! link) are refused with the wording VectorCraft uses, nothing is read into the result and
//! nothing is written outside.
#![cfg(feature = "mcp")]

use std::path::{Path, PathBuf};

use pdfcraft_automation::Automation;
use pdfcraft_automation::file_access::AutomationRoots;
use pdfcraft_automation::mcp::McpServer;
use serde_json::{Value, json};

/// A 1x1 PNG, so the tools that take a picture have something real to read.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
    0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
    0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// A one-page PDF reading `text`, with a 1x1 picture on it.
fn pdf(text: &str) -> Vec<u8> {
    let body = format!("q 100 0 0 100 20 20 cm /Im1 Do Q BT /F1 24 Tf 20 150 Td ({text}) Tj ET");
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> /XObject << /Im1 6 0 R >> >> >>".to_vec(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()).into_bytes(),
        b"<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length 3 >>\nstream\n\xff\x00\x00\nendstream"
            .to_vec(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// A fresh folder holding `in` (the read root, with `a.pdf`, `b.pdf` and `pic.png`), `out` (the
/// write root), `outside` (with `secret.pdf` and a picture) and `in2`.
fn folders(tag: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("pdfcraft-mcp-roots-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    for d in ["in", "out", "outside", "in2"] {
        std::fs::create_dir_all(base.join(d)).unwrap();
    }
    std::fs::write(base.join("in/a.pdf"), pdf("Inside page")).unwrap();
    std::fs::write(base.join("in/b.pdf"), pdf("Second inside")).unwrap();
    std::fs::write(base.join("in/pic.png"), PNG).unwrap();
    std::fs::write(base.join("outside/secret.pdf"), pdf("TOP SECRET")).unwrap();
    std::fs::write(base.join("outside/pic.png"), PNG).unwrap();
    std::fs::write(base.join("in2/c.pdf"), pdf("Sibling")).unwrap();
    base
}

/// A server confined to `in` and `out` (either may be left out).
fn confined(tag: &str, read: bool, write: bool) -> (McpServer, PathBuf) {
    let base = folders(tag);
    let (in_dir, out_dir) = (base.join("in"), base.join("out"));
    let roots = AutomationRoots::new(read.then_some(in_dir.as_path()), write.then_some(out_dir.as_path())).unwrap();
    (McpServer::new(Automation::new().with_automation_roots(roots).with_clock(|| 1_700_000_000)), base)
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn rpc(server: &mut McpServer, method: &str, params: Value) -> Value {
    let line = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string();
    serde_json::from_str(&server.handle_line(&line).unwrap()).unwrap()
}

/// `tools/call` -> (isError, all the text of the reply).
fn call(server: &mut McpServer, name: &str, args: Value) -> (bool, String) {
    let reply = rpc(server, "tools/call", json!({"name": name, "arguments": args}));
    let result = &reply["result"];
    let mut text =
        result["content"].as_array().map(|c| c.iter().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default();
    text.push_str(&result["structuredContent"].to_string());
    text.push_str(&reply["error"].to_string());
    (result["isError"] == true || reply["error"].is_object(), text)
}

fn ok(server: &mut McpServer, name: &str, args: Value) -> Value {
    let reply = rpc(server, "tools/call", json!({"name": name, "arguments": args}));
    assert!(reply["result"]["isError"] == false, "{name} {args}: {reply}");
    reply["result"]["structuredContent"].clone()
}

fn refused(server: &mut McpServer, name: &str, args: Value, why: &str) {
    let (error, text) = call(server, name, args.clone());
    assert!(error && text.contains(why), "{name} {args} should be refused ({why}): {text}");
    assert!(!text.contains("TOP SECRET"), "{name} {args} let the file's text through: {text}");
}

/// The text of document 1, through the resource a client can read it with.
fn text_of(server: &mut McpServer, doc: u64) -> String {
    rpc(server, "resources/read", json!({"uri": format!("pdfcraft://doc/{doc}/text")}))["result"]["contents"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names
}

#[test]
fn files_open_only_from_inside_the_read_root() {
    let (mut server, base) = confined("open", true, true);
    let sep = std::path::MAIN_SEPARATOR;
    ok(&mut server, "doc_open", json!({"path": s(&base.join("in/a.pdf"))}));
    assert!(text_of(&mut server, 1).contains("Inside page"));

    refused(&mut server, "doc_open", json!({"path": s(&base.join("outside/secret.pdf"))}), "outside the read root");
    refused(&mut server, "doc_open", json!({"path": format!("{}{sep}..{sep}outside{sep}secret.pdf", s(&base.join("in")))}), "outside the read root");
    // A sibling whose name starts with the root's name is not inside it.
    refused(&mut server, "doc_open", json!({"path": s(&base.join("in2/c.pdf"))}), "outside the read root");
    // The write root is not readable: the two kinds of access are separate.
    std::fs::write(base.join("out/d.pdf"), pdf("Written")).unwrap();
    refused(&mut server, "doc_open", json!({"path": s(&base.join("out/d.pdf"))}), "outside the read root");
    // A relative path is the working directory's (the crate's folder here): outside.
    refused(&mut server, "doc_open", json!({"path": "Cargo.toml"}), "outside the read root");
    // Nothing was opened by the refused calls.
    assert_eq!(ok(&mut server, "doc_list", json!({}))["documents"].as_array().unwrap().len(), 1);
}

#[test]
fn tools_that_read_a_path_are_held_to_the_read_root() {
    let (mut server, base) = confined("reads", true, true);
    ok(&mut server, "doc_open", json!({"path": s(&base.join("in/a.pdf"))}));
    let (secret, pic, gone) = (s(&base.join("outside/secret.pdf")), s(&base.join("outside/pic.png")), s(&base.join("outside/gone.bin")));
    let out = s(&base.join("out/result.pdf"));
    for (tool, args) in [
        ("page_insert_file", json!({"doc": 1, "path": secret, "at": 1})),
        ("page_replace", json!({"doc": 1, "pages": [1], "path": secret})),
        ("doc_combine", json!({"paths": [s(&base.join("in/a.pdf")), secret], "out": out})),
        ("doc_create_multiple", json!({"paths": [secret], "mode": "combine", "out": out})),
        ("doc_create", json!({"from": "images", "paths": [pic]})),
        ("stamp_custom", json!({"doc": 1, "page": 1, "path": secret, "at": [10, 10]})),
        ("page_add_image", json!({"doc": 1, "page": 1, "path": pic})),
        ("image_edit", json!({"doc": 1, "page": 1, "image": 1, "action": "replace", "path": pic})),
        ("comment_add", json!({"doc": 1, "page": 1, "type": "attachment", "at": [10, 10], "path": secret})),
        ("fill_sign_add", json!({"doc": 1, "page": 1, "type": "signature", "at": [10, 10], "path": pic})),
        ("form_set_image", json!({"doc": 1, "field": "f", "path": pic})),
        ("doc_import_data", json!({"doc": 1, "path": secret})),
        ("doc_watermark", json!({"doc": 1, "file": pic})),
        ("doc_background", json!({"doc": 1, "file": pic})),
        ("form_merge_data", json!({"paths": [secret], "path": out})),
        ("ocr_recognize_files", json!({"paths": [secret], "folder": s(&base.join("out/ocr"))})),
        ("sign_trust", json!({"paths": [secret]})),
        ("sign_document", json!({"doc": 1, "id": secret, "out": out})),
        ("text_font_import", json!({"doc": 1, "page": 1, "path": gone})),
        ("text_font_files", json!({"directories": [s(&base.join("outside"))]})),
    ] {
        refused(&mut server, tool, args, "outside the read root");
    }
    // The refused calls changed neither the document nor the disk.
    assert!(!text_of(&mut server, 1).contains("TOP SECRET"));
    assert_eq!(ok(&mut server, "doc_list", json!({}))["documents"].as_array().unwrap().len(), 1);
    assert_eq!(names(&base.join("out")), Vec::<String>::new());

    // One file per output: a file that can't be read is reported in its own entry, with the same refusal.
    let (_, text) = call(&mut server, "doc_create_multiple", json!({"paths": [secret], "mode": "separate", "out_dir": s(&base.join("out/sep"))}));
    assert!(text.contains("outside the read root") && !text.contains("TOP SECRET"), "{text}");
    assert!(names(&base.join("out/sep")).is_empty());
    let (_, text) = call(&mut server, "action_run", json!({"paths": [secret], "folder": s(&base.join("out/act")), "action": "Add Page Numbers"}));
    assert!(text.contains("outside the read root") && !text.contains("TOP SECRET"), "{text}");
    assert!(names(&base.join("out/act")).is_empty());
    std::fs::remove_dir_all(base.join("out/act")).unwrap();

    // The same tools work on files inside the root.
    std::fs::remove_dir_all(base.join("out/sep")).unwrap();
    ok(&mut server, "page_insert_file", json!({"doc": 1, "path": s(&base.join("in/b.pdf")), "at": 2}));
    assert!(text_of(&mut server, 1).contains("Second inside"));
    ok(&mut server, "page_add_image", json!({"doc": 1, "page": 1, "path": s(&base.join("in/pic.png"))}));
    ok(&mut server, "doc_combine", json!({"paths": [s(&base.join("in/a.pdf")), s(&base.join("in/b.pdf"))], "out": out}));
    assert_eq!(names(&base.join("out")), ["result.pdf"]);
    // Without a folder to list, the installed fonts are outside the read root: it is searched instead.
    ok(&mut server, "text_font_files", json!({}));
    ok(&mut server, "text_font_files", json!({"directories": [s(&base.join("in"))]}));
}

#[test]
fn tools_that_write_a_path_are_held_to_the_write_root() {
    let (mut server, base) = confined("writes", true, true);
    let input = |name: &str| s(&base.join("in").join(name));
    ok(&mut server, "doc_open", json!({"path": input("a.pdf")}));
    ok(&mut server, "doc_open", json!({"path": input("b.pdf")}));
    let writes = |dir: &str| -> Vec<(&'static str, Value)> {
        let file = |name: &str| s(&base.join(dir).join(name));
        vec![
            ("doc_save", json!({"doc": 1, "path": file("saved.pdf")})),
            ("page_extract", json!({"doc": 1, "pages": [1], "out": file("extract.pdf")})),
            ("page_extract", json!({"doc": 1, "pages": [1], "separate": true, "out_dir": file("pages")})),
            ("doc_combine", json!({"paths": [input("a.pdf"), input("b.pdf")], "out": file("combined.pdf")})),
            ("doc_create_multiple", json!({"paths": [input("a.pdf")], "mode": "combine", "out": file("multiple.pdf")})),
            ("doc_create_multiple", json!({"paths": [input("a.pdf")], "mode": "separate", "out_dir": file("separate")})),
            ("doc_split", json!({"doc": 1, "every": 1, "out_dir": file("parts")})),
            ("doc_reduce", json!({"doc": 1, "path": file("reduced.pdf")})),
            ("doc_optimize", json!({"doc": 1, "path": file("optimized.pdf")})),
            ("doc_print", json!({"doc": 1, "path": file("printed.pdf")})),
            ("doc_export_text", json!({"doc": 1, "path": file("text.txt")})),
            ("doc_export_office", json!({"doc": 1, "path": file("export.docx")})),
            ("doc_export_data", json!({"doc": 1, "path": file("data.xfdf")})),
            ("doc_export_images", json!({"doc": 1, "folder": file("renders")})),
            ("doc_export_all_images", json!({"doc": 1, "folder": file("images")})),
            ("doc_export_cbz", json!({"doc": 1, "path": file("comic.cbz")})),
            ("image_save", json!({"doc": 1, "page": 1, "image": 1, "path": file("image.png")})),
            ("measure_export", json!({"doc": 1, "out": file("measurements.csv")})),
            ("comments_summarize", json!({"doc": 1, "out": file("summary.pdf")})),
            ("accessibility_report", json!({"doc": 1, "path": file("report.html")})),
            ("accessibility_review_record", json!({"before": 1, "after": 2, "path": file("record.json")})),
            ("doc_compare_report", json!({"doc": 1, "other": 2, "path": file("compare.pdf")})),
            ("form_merge_data", json!({"paths": [input("a.pdf")], "path": file("merged.csv")})),
            ("action_run", json!({"paths": [input("a.pdf")], "folder": file("actions"), "action": "Add Page Numbers"})),
            ("ocr_recognize_files", json!({"paths": [input("a.pdf")], "folder": file("ocr")})),
            ("sign_id_create", json!({"name": "Test", "password": "secret12", "path": file("id.p12"), "key": "p256"})),
        ]
    };

    for (tool, args) in writes("outside") {
        refused(&mut server, tool, args, "outside the write root");
    }
    // The read root is not writable either.
    for (tool, args) in writes("in") {
        refused(&mut server, tool, args, "outside the write root");
    }
    // `..` out of the write root, and a sibling that shares its prefix.
    let sep = std::path::MAIN_SEPARATOR;
    refused(
        &mut server,
        "doc_save",
        json!({"doc": 1, "path": format!("{}{sep}..{sep}outside{sep}up.pdf", s(&base.join("out")))}),
        "outside the write root",
    );
    std::fs::create_dir_all(base.join("out2")).unwrap();
    refused(&mut server, "doc_save", json!({"doc": 1, "path": s(&base.join("out2/x.pdf"))}), "outside the write root");
    // Saving in place writes to the folder the file was opened from: the read root, not the write root.
    refused(&mut server, "doc_save", json!({"doc": 1}), "outside the write root");

    assert_eq!(names(&base.join("outside")), ["pic.png", "secret.pdf"], "nothing was written outside the roots");
    assert_eq!(names(&base.join("in")), ["a.pdf", "b.pdf", "pic.png"], "nothing was written to the read root");
    assert!(names(&base.join("out2")).is_empty());
    assert!(names(&base.join("out")).is_empty());

    // The same calls land in the write root.
    for (tool, args) in writes("out") {
        let (error, text) = call(&mut server, tool, args.clone());
        // (Printing to a PDF and the like may not apply to this fixture: a refusal for the roots is the failure.)
        assert!(!text.contains("automation path rejected") && !text.contains("automation filesystem access"), "{tool} {args}: {text}");
        let _ = error;
    }
    assert!(base.join("out/saved.pdf").is_file() && base.join("out/combined.pdf").is_file() && base.join("out/text.txt").is_file());
    assert_eq!(names(&base.join("outside")), ["pic.png", "secret.pdf"]);
}

#[test]
fn signing_writes_and_reads_only_inside_the_roots() {
    // One folder for both roots, so a digital ID can be made and then read back.
    let base = folders("signing");
    let work = base.join("work");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(work.join("a.pdf"), pdf("Work page")).unwrap();
    let roots = AutomationRoots::new(Some(&work), Some(&work)).unwrap();
    let mut server = McpServer::new(Automation::new().with_automation_roots(roots).with_clock(|| 1_700_000_000));
    ok(&mut server, "doc_open", json!({"path": s(&work.join("a.pdf"))}));
    ok(&mut server, "sign_id_create", json!({"name": "Test", "password": "secret12", "path": s(&work.join("id.p12")), "key": "p256"}));
    refused(
        &mut server,
        "sign_id_create",
        json!({"name": "Test", "password": "secret12", "path": s(&base.join("outside/id.p12")), "key": "p256"}),
        "outside the write root",
    );
    refused(
        &mut server,
        "sign_document",
        json!({"doc": 1, "id": s(&work.join("id.p12")), "password": "secret12", "out": s(&base.join("outside/signed.pdf"))}),
        "outside the write root",
    );
    ok(&mut server, "sign_document", json!({"doc": 1, "id": s(&work.join("id.p12")), "password": "secret12", "out": s(&work.join("signed.pdf"))}));
    assert!(work.join("signed.pdf").is_file());
    assert_eq!(names(&base.join("outside")), ["pic.png", "secret.pdf"]);
}

#[test]
fn a_root_left_out_grants_none_of_its_access() {
    let (mut server, base) = confined("read-only", true, false);
    ok(&mut server, "doc_open", json!({"path": s(&base.join("in/a.pdf"))}));
    for target in ["out/doc.pdf", "in/doc.pdf"] {
        refused(&mut server, "doc_save", json!({"doc": 1, "path": s(&base.join(target))}), "write authority is absent");
    }
    refused(&mut server, "doc_save", json!({"doc": 1}), "write authority is absent");
    refused(&mut server, "doc_export_text", json!({"doc": 1, "path": s(&base.join("out/t.txt"))}), "write authority is absent");
    refused(&mut server, "page_extract", json!({"doc": 1, "pages": [1], "out": s(&base.join("out/e.pdf"))}), "write authority is absent");
    refused(&mut server, "doc_split", json!({"doc": 1, "every": 1, "out_dir": s(&base.join("out/parts"))}), "write authority is absent");
    assert!(names(&base.join("out")).is_empty() && names(&base.join("in")) == ["a.pdf", "b.pdf", "pic.png"]);
    // A tool that gives its result back instead of writing it still can.
    ok(&mut server, "page_extract", json!({"doc": 1, "pages": [1]}));

    let (mut server, base) = confined("write-only", false, true);
    refused(&mut server, "doc_open", json!({"path": s(&base.join("in/a.pdf"))}), "read authority is absent");
    refused(
        &mut server,
        "doc_combine",
        json!({"paths": [s(&base.join("in/a.pdf")), s(&base.join("in/b.pdf"))], "out": s(&base.join("out/c.pdf"))}),
        "read authority is absent",
    );
    refused(&mut server, "text_font_files", json!({}), "read authority is absent");
    ok(&mut server, "doc_create", json!({"from": "blank", "pages": 1, "width": 200, "height": 300}));
    ok(&mut server, "doc_save", json!({"doc": 1, "path": s(&base.join("out/blank.pdf"))}));
    assert!(base.join("out/blank.pdf").is_file());
}

#[test]
fn links_out_of_a_root_are_refused() {
    let (mut server, base) = confined("links", true, true);
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(base.join("outside"), base.join("out/away")).is_ok()
        && std::os::unix::fs::symlink(base.join("outside/secret.pdf"), base.join("in/leak.pdf")).is_ok()
        && std::os::unix::fs::symlink(base.join("outside/nowhere.pdf"), base.join("out/dangling.pdf")).is_ok();
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_dir(base.join("outside"), base.join("out/away")).is_ok()
        && std::os::windows::fs::symlink_file(base.join("outside/secret.pdf"), base.join("in/leak.pdf")).is_ok()
        && std::os::windows::fs::symlink_file(base.join("outside/nowhere.pdf"), base.join("out/dangling.pdf")).is_ok();
    #[cfg(not(any(unix, windows)))]
    let linked = false;
    // Windows needs Developer Mode or elevation to make links.
    if !linked {
        eprintln!("skipped: symbolic links can't be made here");
        return;
    }
    ok(&mut server, "doc_open", json!({"path": s(&base.join("in/a.pdf"))}));
    refused(&mut server, "doc_open", json!({"path": s(&base.join("in/leak.pdf"))}), "outside the read root");
    refused(&mut server, "doc_save", json!({"doc": 1, "path": s(&base.join("out/away/doc.pdf"))}), "outside the write root");
    refused(&mut server, "doc_save", json!({"doc": 1, "path": s(&base.join("out/away/deeper/doc.pdf"))}), "outside the write root");
    refused(&mut server, "doc_save", json!({"doc": 1, "path": s(&base.join("out/dangling.pdf"))}), "a link that leads nowhere");
    assert_eq!(names(&base.join("outside")), ["pic.png", "secret.pdf"]);
}

#[test]
fn without_roots_nothing_is_confined() {
    let base = folders("none");
    let mut server = McpServer::new(Automation::new());
    ok(&mut server, "doc_open", json!({"path": s(&base.join("outside/secret.pdf"))}));
    assert!(text_of(&mut server, 1).contains("TOP SECRET"));
    ok(&mut server, "doc_save", json!({"doc": 1, "path": s(&base.join("in2/copy.pdf"))}));
    assert!(base.join("in2/copy.pdf").is_file());
}

#[test]
fn the_roots_apply_on_top_of_a_root() {
    let base = folders("both");
    let roots = AutomationRoots::new(Some(&base.join("in")), Some(&base.join("out"))).unwrap();
    // `--root` resolves relative paths inside `in`, which the read root allows and the write root does not.
    let mut server = McpServer::new(Automation::new().with_root(base.join("in")).unwrap().with_automation_roots(roots));
    ok(&mut server, "doc_open", json!({"path": "a.pdf"}));
    refused(&mut server, "doc_save", json!({"doc": 1, "path": "copy.pdf"}), "outside the write root");
    refused(&mut server, "doc_open", json!({"path": "../outside/secret.pdf"}), "outside the allowed directory");
    assert!(!base.join("in/copy.pdf").exists());
    // The write root alone is not enough: the other root still has its say.
    refused(&mut server, "doc_save", json!({"doc": 1, "path": "../out/copy.pdf"}), "outside the allowed directory");
}
