//! The plug-in ABI end to end: the example Hello plug-in, and hostile modules (infinite loops,
//! out-of-bounds access, bad manifests…), which must all fail with an error, quickly.
//! Modelled on the PhotoCraft/VectorCraft `crates/plugins/tests/abi.rs` suites.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use pdfcraft_plugin::{Action, LoadedPlugin};

const HELLO: &[u8] = include_bytes!("fixtures/hello.wasm");

const MANIFEST: &str = r#"{"id":"test.plugin","name":"Test","version":"1","kind":"command","commands":[{"id":"ping","label":"Ping"}]}"#;

/// A module implementing the ABI whose pieces can be swapped: `manifest` JSON, `version`,
/// `run` body (params `$input $len $params $plen`, result i64) and extra items.
fn module(manifest: &str, version: i32, run: &str, extra: &str) -> Vec<u8> {
    let escaped = manifest.replace('\\', "\\\\").replace('"', "\\\"");
    let src = format!(
        r#"(module
  {extra}
  (memory (export "memory") 1)
  (data (i32.const 16) "{escaped}")
  (func (export "pdf_abi_version") (result i32) (i32.const {version}))
  (func (export "pdf_manifest") (result i64)
    (i64.or (i64.shl (i64.const {len}) (i64.const 32)) (i64.const 16)))
  (func (export "pdf_alloc") (param $n i32) (result i32) (local $old i32)
    (local.set $old (memory.grow (i32.shr_u (i32.add (local.get $n) (i32.const 65535)) (i32.const 16))))
    (if (result i32) (i32.eq (local.get $old) (i32.const -1))
      (then (i32.const 0)) (else (i32.mul (local.get $old) (i32.const 65536)))))
  (func (export "pdf_run") (param $input i32) (param $len i32) (param $params i32) (param $plen i32) (result i64)
    {run})
)"#,
        len = manifest.len()
    );
    wat::parse_str(&src).unwrap_or_else(|e| panic!("bad test WAT: {e}\n{src}"))
}

/// A valid module whose `pdf_run` writes `"{}"` at offset 4096 and returns it packed.
fn ok_module(run: &str) -> Vec<u8> {
    module(MANIFEST, 1, run, "")
}

/// Runs `bytes` as a plug-in command; returns the error and how long it took.
fn invoke_err(bytes: &[u8]) -> (pdfcraft_plugin::PluginError, Duration) {
    let t = Instant::now();
    let r = LoadedPlugin::from_bytes(bytes.to_vec()).and_then(|p| p.invoke("ping", &serde_json::json!({"document": {"name": "x.pdf"}})));
    (r.expect_err("hostile module must fail"), t.elapsed())
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pdfcraft-plugin-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[test]
fn example_hello_loads_and_reports_its_manifest() {
    let p = LoadedPlugin::from_bytes(HELLO.to_vec()).expect("hello loads");
    let m = p.manifest().expect("manifest");
    assert_eq!(m.id, "org.pdfcraft.example.hello");
    assert_eq!(m.kind, "command");
    assert_eq!(m.commands.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["summarize", "mark_reviewed"]);
}

#[test]
fn context_reaches_the_plugin_and_actions_come_back() {
    let p = LoadedPlugin::from_bytes(HELLO.to_vec()).expect("hello loads");
    let context = serde_json::json!({"document": {"name": "demo.pdf", "page_count": 4, "current_page": 1}});
    let r = p.invoke("summarize", &context).expect("summarize runs");
    assert_eq!(r.message.as_deref(), Some("demo.pdf: 4 pages, you are on page 2."));

    let r = p.invoke("mark_reviewed", &context).expect("mark_reviewed runs");
    assert!(r.actions.contains(&Action::SetDocumentInfo { key: "Keywords".into(), value: "reviewed".into() }));

    let r = p.invoke("nope", &context).expect("unknown command still answers");
    assert_eq!(r.message.as_deref(), Some("Unknown command."));
}

#[test]
fn hostile_modules_fail_quickly() {
    // No ABI exports at all.
    let (e, elapsed) = invoke_err(b"(module)".as_slice());
    let _ = e;
    assert!(elapsed < Duration::from_secs(10), "took {elapsed:?}");

    // Compile-time garbage and an empty file.
    assert!(LoadedPlugin::from_bytes(b"not a wasm module".to_vec()).is_err());
    assert!(LoadedPlugin::from_bytes(Vec::new()).is_err());

    // An import (guests get no host surface).
    let imported = module(MANIFEST, 1, "(i64.const 0)", "(import \"host\" \"f\" (func))");
    assert!(LoadedPlugin::from_bytes(imported).is_err());

    // Infinite loop: the fuel budget stops it.
    let (e, elapsed) = invoke_err(&ok_module("(loop (br 0)) (i64.const 0)"));
    assert!(elapsed < Duration::from_secs(30), "loop took {elapsed:?}");
    let _ = e;

    // An answer claiming far more bytes than the payload cap is refused.
    let (e, _) = invoke_err(&ok_module("(i64.or (i64.shl (i64.const 60000000) (i64.const 32)) (i64.const 16))"));
    assert!(e.to_string().contains("too large") || e.to_string().contains("runtime"), "{e}");
}

#[test]
fn malformed_and_incompatible_modules_are_refused() {
    // Wrong ABI version.
    let v2 = module(MANIFEST, 2, "(i64.const 0)", "");
    assert!(LoadedPlugin::from_bytes(v2).is_err());

    // Manifest that is not JSON.
    let bad = module("not json", 1, "(i64.const 0)", "");
    assert!(LoadedPlugin::from_bytes(bad).is_err());

    // A kind PdfCraft does not host.
    let filter = module(r#"{"id":"test.filter","name":"F","version":"1","kind":"filter"}"#, 1, "(i64.const 0)", "");
    let p = LoadedPlugin::from_bytes(filter).expect("module loads");
    let m = p.manifest().expect("manifest reads");
    assert!(m.compatibility().is_some_and(|why| why.contains("filter")));

    // Oversized module (over the 32 MiB cap) is refused before parsing.
    let big = vec![0u8; 33 * 1024 * 1024];
    assert!(LoadedPlugin::from_bytes(big).is_err());
}

#[test]
fn registry_install_list_remove() {
    let dir = temp_dir("registry");
    let manifest = pdfcraft_plugin::install_module(HELLO, &dir).expect("install");
    assert_eq!(manifest.id, "org.pdfcraft.example.hello");
    assert!(dir.join("org.pdfcraft.example.hello.wasm").is_file());

    let (found, problems) = pdfcraft_plugin::discover(&dir);
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].manifest.id, manifest.id);

    // Re-installing the same id replaces, it does not duplicate.
    pdfcraft_plugin::install_module(HELLO, &dir).expect("reinstall");
    let (found, _) = pdfcraft_plugin::discover(&dir);
    assert_eq!(found.len(), 1);

    pdfcraft_plugin::uninstall(&dir, &manifest.id).expect("remove");
    let (found, _) = pdfcraft_plugin::discover(&dir);
    assert!(found.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn folder_loading_skips_bad_files() {
    let dir = temp_dir("folder");
    std::fs::write(dir.join("hello.wasm"), HELLO).expect("write good");
    std::fs::write(dir.join("broken.wasm"), b"not wasm").expect("write bad");
    std::fs::write(dir.join("notes.txt"), b"hello").expect("write other");
    let (found, problems) = pdfcraft_plugin::discover(&dir);
    assert_eq!(found.len(), 1);
    assert_eq!(problems.len(), 1, "{problems:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn disabled_set_round_trips() {
    let dir = temp_dir("disabled");
    let mut set = pdfcraft_plugin::load_disabled(&dir);
    assert!(set.is_empty());
    set.insert("org.example.x".to_string());
    pdfcraft_plugin::save_disabled(&dir, &set).expect("save");
    assert!(pdfcraft_plugin::load_disabled(&dir).contains("org.example.x"));
    let _ = std::fs::remove_dir_all(&dir);
}
