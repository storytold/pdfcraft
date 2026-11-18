//! Plug-ins (RFC-0046) through the tool table: installing, listing and removing them, and running
//! one as one undo step that is put back when the run fails, pinned to its document and limited to
//! the tools PdfCraft allows plug-ins.

use std::path::{Path, PathBuf};

use pdfcraft_automation::plugins::craft_plugin::bundle::Bundle;
use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

/// A minimal `n`-page document; page `i` shows "Page i+1".
fn fixture(n: usize) -> Vec<u8> {
    let mut objs: Vec<Vec<u8>> = vec![b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 200 300] >>", kids.join(" ")).into_bytes());
    objs.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
    for i in 0..n {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * i).into_bytes());
        let body = format!("BT /F1 24 Tf 20 150 Td (Page {}) Tj ET", i + 1);
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()).into_bytes());
    }
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

/// A fresh folder with `a.pdf` (3 pages) and `b.pdf` (2 pages).
fn workdir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pdfcraft-plugins-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.pdf"), fixture(3)).unwrap();
    std::fs::write(dir.join("b.pdf"), fixture(2)).unwrap();
    dir
}

fn auto(dir: &Path) -> Automation {
    Automation::new().with_root(dir).unwrap().with_clock(|| 1_700_000_000)
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

fn err(a: &mut Automation, tool: &str, args: Value) -> String {
    a.call(tool, &args).unwrap_err().to_string()
}

/// How a test module ends after its calls.
#[derive(Clone, Copy)]
enum End {
    /// Returns 0: the run succeeds.
    Ok,
    /// Traps (`unreachable`): the run fails.
    Trap,
    /// Loops until its instruction budget runs out.
    Spin,
}

/// WebAssembly text for `s`'s bytes, each escaped.
fn wat_bytes(s: &str) -> String {
    s.bytes().map(|b| format!("\\{b:02x}")).collect()
}

/// A plug-in module that makes `calls` (tool or command id, params JSON) in order. When a call is
/// refused or fails, the reply becomes the run's output and the run stops: successfully with
/// `failed_ok`, else with status 1. Then it ends as `end` says.
fn module(calls: &[(&str, &str)], failed_ok: bool, end: End) -> Vec<u8> {
    let mut data = String::new();
    let mut body = String::new();
    let mut at = 1024usize;
    for (command, params) in calls {
        let (c, p) = (at, at + command.len());
        at = p + params.len() + 16;
        data.push_str(&format!("(data (i32.const {c}) \"{}\")\n(data (i32.const {p}) \"{}\")\n", wat_bytes(command), wat_bytes(params)));
        body.push_str(&format!(
            "(local.set $s (call $call (i32.const {c}) (i32.const {}) (i32.const {p}) (i32.const {})))
             (if (local.get $s) (then
               (local.set $n (call $reply_read (i32.const 16384) (i32.const 32768)))
               (call $set_output (i32.const 16384) (local.get $n))
               (return (i32.const {}))))\n",
            command.len(),
            params.len(),
            i32::from(!failed_ok)
        ));
    }
    let end = match end {
        End::Ok => "(i32.const 0)",
        End::Trap => "unreachable",
        End::Spin => "(loop $l (br $l)) (i32.const 0)",
    };
    let text = format!(
        r#"(module
          (import "craft" "call" (func $call (param i32 i32 i32 i32) (result i32)))
          (import "craft" "reply_read" (func $reply_read (param i32 i32) (result i32)))
          (import "craft" "set_output" (func $set_output (param i32 i32)))
          (memory (export "memory") 1)
          {data}
          (func (export "craft_run") (param i32) (result i32)
            (local $s i32) (local $n i32)
            {body}
            {end}))"#
    );
    wat::parse_str(&text).unwrap()
}

/// A bundle of plug-in `id` for PdfCraft, asking for `access` to `commands` (JSON array).
fn bundle(id: &str, access: &str, commands: &str, wasm: Vec<u8>) -> Vec<u8> {
    let manifest = format!(
        r#"{{"id": "{id}", "name": "Test {id}", "version": "1.0.0", "main": "p.wasm", "apps": {{"pdfcraft": {{"api": 6}}}},
            "permissions": {{"access": "{access}", "commands": {commands}}}, "commands": [{{"id": "run", "label": "Run"}}]}}"#
    );
    Bundle { manifest, wasm, assets: Vec::new() }.encode()
}

/// Install `bytes` (written into `dir` as `<id>.craftplugin`) through the tools.
fn install(a: &mut Automation, dir: &Path, id: &str, bytes: &[u8]) -> Value {
    let name = format!("{id}.craftplugin");
    std::fs::write(dir.join(&name), bytes).unwrap();
    ok(a, "plugin_install", json!({ "path": name }))
}

fn open(a: &mut Automation, file: &str) -> u64 {
    ok(a, "doc_open", json!({ "path": file }))["doc"].as_u64().unwrap()
}

fn rotations(a: &mut Automation, doc: u64) -> Vec<i64> {
    let info = ok(a, "doc_info", json!({ "doc": doc }));
    info["pages"].as_array().unwrap().iter().map(|p| p["rotation"].as_i64().unwrap_or(0)).collect()
}

#[test]
fn a_write_plugin_runs_real_tools_as_one_undo_step() {
    let dir = workdir("write");
    let mut a = auto(&dir);
    let doc = open(&mut a, "a.pdf");
    let calls = [("page_rotate", r#"{"pages": [1], "degrees": 90}"#), ("page.delete", r#"{"pages": [3]}"#)];
    let b = bundle("org.example.tidy", "write", r#"["page_rotate", "page.delete"]"#, module(&calls, false, End::Ok));
    install(&mut a, &dir, "tidy", &b);
    let ran = ok(&mut a, "plugin_run", json!({ "id": "org.example.tidy" }));
    assert_eq!(ran["edited"], json!(true), "{ran}");
    assert_eq!(ran["document"]["pages"], json!(2));
    assert_eq!(ran["document"]["undo"], json!("Plug-in: Test org.example.tidy"), "one step named after the plug-in");
    assert_eq!(rotations(&mut a, doc), vec![90, 0]);
    let undone = ok(&mut a, "edit_undo", json!({ "doc": doc }));
    assert_eq!(undone["undone"], json!("Plug-in: Test org.example.tidy"));
    assert_eq!(undone["document"]["pages"], json!(3), "one Undo takes the whole run back");
    assert_eq!(undone["document"]["undo"], Value::Null, "and nothing before it was folded in");
    assert_eq!(rotations(&mut a, doc), vec![0, 0, 0]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_read_plugin_that_edits_is_refused_and_the_document_is_unchanged() {
    let dir = workdir("read");
    let mut a = auto(&dir);
    let doc = open(&mut a, "a.pdf");
    let calls = [("doc_info", "{}"), ("page_rotate", r#"{"degrees": 90}"#)];
    let b = bundle("org.example.peek", "read", "[]", module(&calls, true, End::Ok));
    install(&mut a, &dir, "peek", &b);
    let ran = ok(&mut a, "plugin_run", json!({ "id": "org.example.peek", "doc": doc }));
    let output = ran["output"].as_str().unwrap_or_default();
    assert!(output.contains("may only read"), "{ran}");
    assert_eq!(ran["edited"], json!(false));
    assert_eq!(ran["document"]["undo"], Value::Null);
    assert_eq!(rotations(&mut a, doc), vec![0, 0, 0]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn refused_tools_files_and_other_documents_are_refused() {
    let dir = workdir("refused");
    let mut a = auto(&dir);
    let doc = open(&mut a, "a.pdf");
    let other = open(&mut a, "b.pdf");
    let cases = [
        ("doc_save", "{}".to_string(), "isn't available to plugins"),
        ("file.save", "{}".to_string(), "isn't available to plugins"),
        ("edit_undo", "{}".to_string(), "isn't available to plugins"),
        ("js_run", r#"{"script": "1"}"#.to_string(), "isn't available to plugins"),
        ("redact_apply", "{}".to_string(), "isn't available to plugins"),
        ("plugin_list", "{}".to_string(), "isn't available to plugins"),
        ("comment_add", r#"{"page": 1, "path": "/etc/passwd"}"#.to_string(), "can't name files"),
        ("page_rotate", format!(r#"{{"doc": {other}, "degrees": 90}}"#), "the document they run on"),
    ];
    for (i, (tool, params, why)) in cases.iter().enumerate() {
        let id = format!("org.example.r{i}");
        let b = bundle(&id, "write", "[]", module(&[(tool, params)], true, End::Ok));
        install(&mut a, &dir, &format!("r{i}"), &b);
        let ran = ok(&mut a, "plugin_run", json!({ "id": id, "doc": doc }));
        assert!(ran["output"].as_str().unwrap_or_default().contains(why), "{tool}: {ran}");
        assert_eq!(ran["edited"], json!(false), "{tool}");
    }
    // A tool outside the manifest's own permissions.
    let b = bundle("org.example.narrow", "write", r#"["bookmark_*"]"#, module(&[("page_rotate", r#"{"degrees": 90}"#)], true, End::Ok));
    install(&mut a, &dir, "narrow", &b);
    let ran = ok(&mut a, "plugin_run", json!({ "id": "org.example.narrow", "doc": doc }));
    assert!(ran["output"].as_str().unwrap_or_default().contains("asked permission for"), "{ran}");
    assert_eq!(rotations(&mut a, doc), vec![0, 0, 0]);
    assert_eq!(rotations(&mut a, other), vec![0, 0]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_plugin_that_traps_or_runs_out_of_fuel_leaves_the_document_unchanged() {
    let dir = workdir("trap");
    let mut a = auto(&dir);
    let doc = open(&mut a, "a.pdf");
    ok(&mut a, "page_rotate", json!({ "doc": doc, "pages": [2], "degrees": 180 }));
    let before = ok(&mut a, "doc_info", json!({ "doc": doc }));
    let calls = [("page_rotate", r#"{"degrees": 90}"#), ("page_delete", r#"{"pages": [1]}"#)];
    for (id, end) in [("org.example.trap", End::Trap), ("org.example.spin", End::Spin)] {
        let b = bundle(id, "write", "[]", module(&calls, false, end));
        install(&mut a, &dir, id, &b);
        let e = err(&mut a, "plugin_run", json!({ "id": id, "doc": doc, "fuel": 1_000_000 }));
        assert!(e.contains(&format!("Test {id}")), "{e}");
        let after = ok(&mut a, "doc_info", json!({ "doc": doc }));
        assert_eq!(after["pages"], before["pages"], "{id}: the document is as it was");
        let summary = ok(&mut a, "doc_list", json!({}))["documents"][0].clone();
        assert_eq!(summary["undo"], json!("Rotate page"), "{id}: the history is as it was");
        assert_eq!(summary["redo"], Value::Null);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn plugins_install_list_and_remove_through_the_store() {
    let dir = workdir("store");
    let settings = dir.join("settings");
    let mut a = Automation::new().with_plugin_store(&settings);
    let b = bundle("org.example.keep", "read", "[]", module(&[], false, End::Ok));
    std::fs::write(dir.join("keep.craftplugin"), &b).unwrap();
    let path = dir.join("keep.craftplugin").to_string_lossy().into_owned();

    // Reviewed first: nothing is installed until the reviewed bundle is confirmed.
    let review = ok(&mut a, "plugin_install", json!({ "path": path, "review": true }));
    assert_eq!(review["id"], json!("org.example.keep"));
    assert_eq!(review["permissions"]["access"], json!("read"));
    assert_eq!(review["kind"], json!("command"));
    assert_eq!(review["inCatalog"], Value::Null);
    assert_eq!(ok(&mut a, "plugin_list", json!({}))["plugins"], json!([]));
    let wrong = err(&mut a, "plugin_install", json!({ "path": path, "sha256": "00" }));
    assert!(wrong.contains("review it again"), "{wrong}");
    let sha = review["sha256"].as_str().unwrap().to_string();
    let installed = ok(&mut a, "plugin_install", json!({ "path": path, "sha256": sha }));
    assert_eq!(installed["installed"]["stored"], json!(true));

    // Another session reads it from the folder.
    let mut b2 = Automation::new().with_plugin_store(&settings);
    let listed = ok(&mut b2, "plugin_list", json!({}));
    assert_eq!(listed["stored"], json!(true));
    assert_eq!(listed["plugins"][0]["id"], json!("org.example.keep"));
    assert_eq!(listed["plugins"][0]["sha256"], json!(sha));

    // A plug-in for another app is refused before anything is written.
    let manifest = r#"{"id": "org.example.other", "name": "Other", "version": "1.0.0", "main": "p.wasm", "apps": {"wordcraft": {"api": 6}}}"#;
    let other = Bundle { manifest: manifest.into(), wasm: module(&[], false, End::Ok), assets: Vec::new() }.encode();
    std::fs::write(dir.join("other.craftplugin"), &other).unwrap();
    let refused = err(&mut b2, "plugin_install", json!({ "path": dir.join("other.craftplugin").to_string_lossy() }));
    assert!(refused.contains("wordcraft"), "{refused}");

    assert_eq!(ok(&mut b2, "plugin_remove", json!({ "id": "org.example.keep" }))["removed"], json!(true));
    assert_eq!(ok(&mut b2, "plugin_remove", json!({ "id": "org.example.keep" }))["removed"], json!(false));
    let mut c = Automation::new().with_plugin_store(&settings);
    assert_eq!(ok(&mut c, "plugin_list", json!({}))["plugins"], json!([]));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_confined_session_reads_bundles_inside_its_root_only() {
    let dir = workdir("confined");
    let outside = workdir("confined-outside");
    std::fs::write(outside.join("x.craftplugin"), bundle("org.example.x", "read", "[]", module(&[], false, End::Ok))).unwrap();
    let mut a = auto(&dir);
    let e = err(&mut a, "plugin_install", json!({ "path": outside.join("x.craftplugin").to_string_lossy() }));
    assert!(e.contains("outside the allowed directory"), "{e}");
    let e = err(&mut a, "plugin_catalog_add", json!({ "url": format!("file://{}/index.json", dir.display()) }));
    assert!(e.contains("confined"), "{e}");
    let e = err(&mut a, "plugin_install", json!({ "repo": "example/tools" }));
    assert!(e.contains("no network access"), "{e}");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&outside);
}
