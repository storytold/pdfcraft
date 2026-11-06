//! Every tool, called with hostile arguments (D2) and with paths that try to leave `--root` (D3).
//!
//! AGENTS.md §3 and §4 treat tool arguments as untrusted — they are the MCP attack surface, and an
//! agent can send any JSON at all. But `tools::check_args` only checks the *shape* of an argument
//! object: that it is an object, that it has no unknown keys, and that the required keys are
//! present and not null. **Types and ranges are not validated there**, so a string where an
//! integer belongs, `i64::MIN` where a page number belongs, or a 100,000-character path all reach
//! the handler. Each handler has to cope on its own, and nothing checked that they all did.
//!
//! These tests call every tool in the table with a zoo of hostile values and require two things:
//!
//! 1. **Nothing panics.** A panic in a tool is a crashed MCP server or CLI (AGENTS.md §4), so
//!    every call must come back as `Ok` or as a `ToolError` the agent can read.
//! 2. **Nothing escapes `--root`.** Every path argument is tried with `..` traversal, absolute
//!    paths, Windows drive and UNC paths, and a planted symbolic link. The call must fail, and
//!    afterwards nothing outside the root may have been created.
//!
//! The second is the mechanized form of the D3 audit. Reading the code says every handler resolves
//! its own paths and that no tool dispatches another internally; this keeps that true as tools are
//! added, which a written finding would not.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use pdfcraft_automation::{Automation, ToolError, tools};
use serde_json::{Value, json};

/// A fresh empty directory for one test.
fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pdfcraft-hostile-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("temp dir");
    d
}

/// Values chosen to break assumptions: wrong types, the ends of the numeric ranges, strings that
/// are empty, enormous, or full of separators and control characters, and containers that are
/// empty, mistyped or deeply nested.
fn zoo() -> Vec<Value> {
    let deep = {
        let mut v = json!(0);
        for _ in 0..64 {
            v = json!([v]);
        }
        v
    };
    vec![
        Value::Null,
        json!(true),
        json!(false),
        json!(0),
        json!(-1),
        json!(1),
        json!(i64::MAX),
        json!(i64::MIN),
        json!(u64::MAX),
        json!(0.5),
        json!(-0.0),
        json!(1e308),
        json!(-1e308),
        json!(""),
        json!(" "),
        json!("0"),
        json!("-1"),
        json!("null"),
        json!("\0"),
        json!("\u{202E}gnp.txe"), // right-to-left override: a name that displays reversed
        json!("💥🙈"),
        json!("%n%s%p"),
        json!("a".repeat(100_000)),
        json!("../".repeat(64)),
        json!([]),
        json!([null]),
        json!([i64::MIN, i64::MAX]),
        json!(["", "../.."]),
        json!({}),
        json!({ "unexpected": 1 }),
        deep,
    ]
}

/// Paths that must never resolve inside the root. Absolute ones point into the test's own temp
/// `base` (the root's parent), never at real system files or network shares, so a confinement
/// bug can only touch what the test made. `linked`: the root holds `link-out`, a symbolic link
/// to `base`.
fn escapes(base: &std::path::Path, linked: bool) -> Vec<String> {
    let mut out: Vec<String> = vec![
        "../outside.pdf".into(),
        "../../outside.pdf".into(),
        "../".repeat(40) + "outside.pdf",
        "sub/../../outside.pdf".into(),
        "./././../outside.pdf".into(),
        base.join("outside.pdf").to_string_lossy().into_owned(),
        base.join("new-outside.pdf").to_string_lossy().into_owned(),
        base.join("sub").join("new-outside.pdf").to_string_lossy().into_owned(),
    ];
    if linked {
        out.push("link-out/outside.pdf".into());
    }
    // Backslashes separate only on Windows; elsewhere this is an ordinary file name in the root.
    if cfg!(windows) {
        out.push(r"..\..\outside.pdf".into());
    }
    out
}

/// A symbolic link to a folder, where the system allows one (Windows needs Developer Mode or an
/// administrator for it, #948).
fn dir_symlink(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(target, link)
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err(std::io::Error::other(format!("no symbolic links here: {} {}", target.display(), link.display())))
    }
}

/// The properties of `def` whose values are paths, by name and whether they are a list.
fn path_props(def: &pdfcraft_automation::ToolDef) -> Vec<(String, bool)> {
    let Some(props) = def.input_schema["properties"].as_object() else { return Vec::new() };
    props
        .iter()
        .filter(|(name, spec)| {
            // `out` (an output file) and `image` (a picture to place) are paths when they are
            // strings; `image` is an image number elsewhere, which the integer check skips.
            let is_pathish = matches!(name.as_str(), "path" | "paths" | "folder" | "out_dir" | "file" | "out" | "image")
                || spec["description"].as_str().is_some_and(|d| d.starts_with("A file path"));
            // `file_page` is a page number inside a file, not a path.
            is_pathish && spec["type"] != "integer"
        })
        .map(|(name, spec)| (name.clone(), spec["type"] == "array"))
        .collect()
}

/// An arguments object giving every required property a plausible value, so a hostile value for
/// one property is not masked by a "missing argument" error for another.
///
/// `doc` gets a **real** open document when one is given. Without that, almost every tool fails at
/// `self.doc()` before it ever looks at a path, and a confinement test would pass vacuously — as
/// an earlier version of this file did, even with `resolve` disabled entirely.
fn plausible_with(def: &pdfcraft_automation::ToolDef, doc: Option<u64>) -> serde_json::Map<String, Value> {
    let mut obj = plausible(def);
    if let Some(id) = doc
        && def.input_schema["properties"].as_object().is_some_and(|p| p.contains_key("doc"))
    {
        obj.insert("doc".into(), json!(id));
    }
    obj
}

fn plausible(def: &pdfcraft_automation::ToolDef) -> serde_json::Map<String, Value> {
    let mut obj = serde_json::Map::new();
    let props = def.input_schema["properties"].as_object().cloned().unwrap_or_default();
    for req in def.input_schema["required"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        let spec = props.get(req).cloned().unwrap_or(Value::Null);
        let value = match spec["type"].as_str() {
            Some("integer") => json!(1),
            Some("number") => json!(1.0),
            Some("boolean") => json!(false),
            Some("array") => json!([]),
            Some("object") => json!({}),
            _ => json!("in.pdf"),
        };
        obj.insert(req.to_string(), value);
    }
    obj
}

/// Run `call`, turning a panic into a message instead of unwinding out of the test.
fn guarded(a: &mut Automation, tool: &str, args: &Value) -> std::result::Result<(), String> {
    let result = catch_unwind(AssertUnwindSafe(|| a.call(tool, args)));
    match result {
        Ok(Ok(_)) => Ok(()),
        // Any ToolError is a good outcome: the agent is told what was wrong.
        Ok(Err(ToolError::UnknownTool(_) | ToolError::InvalidArgs(_) | ToolError::Failed(_))) => Ok(()),
        Err(panic) => {
            let what = panic
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic".into());
            Err(format!("{tool} panicked on {args}: {what}"))
        }
    }
}

/// A new one-page blank document, by id.
fn blank_doc(a: &mut Automation) -> u64 {
    let created = a.call("doc_create", &json!({ "from": "blank", "pages": 1 })).expect("create a blank document");
    created
        .into_iter()
        .find_map(|c| match c {
            pdfcraft_automation::Content::Json(v) => v["doc"].as_u64(),
            pdfcraft_automation::Content::Png { .. } => None,
        })
        .expect("the new document has an id")
}

/// Delete everything inside `dir`, so one tool never sees files an earlier tool wrote: a digital ID
/// that `sign_id_create` wrote to the `in.pdf` placeholder would stop every later `sign_document`
/// call at its decrypt, before the argument under test.
///
/// The entries are reached through the canonical path, as the tools reach them: on Windows that
/// is a `\\?\` path, whose names are taken literally. Through the plain path a file the tools
/// named `" "` would lose its trailing space and name `dir` itself, which would then be deleted.
fn empty_dir(dir: &Path) {
    let dir = dir.canonicalize().expect("the root exists");
    for entry in std::fs::read_dir(&dir).expect("read the root") {
        let entry = entry.unwrap_or_else(|e| panic!("list {}: {e}", dir.display()));
        let path = entry.path();
        let is_dir = entry.file_type().unwrap_or_else(|e| panic!("file type of {}: {e}", path.display())).is_dir();
        let removed = if is_dir { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
        removed.unwrap_or_else(|e| panic!("remove {} before the next tool: {e}", path.display()));
    }
}

/// Real values for the arguments a tool checks before the others, so a hostile value reaches the
/// argument it is meant for instead of failing on a placeholder. `sign_document` checks its
/// digital ID first and its document second: this ID opens with its password, and each call gets
/// a fresh document (which the caller closes), since a successful call signs it. The ID is a test
/// key with a low key derivation count, so the hundreds of calls that open it stay fast.
fn fixture(a: &mut Automation, root: &Path, tool: &str) -> serde_json::Map<String, Value> {
    match tool {
        "sign_document" => {
            let id = Path::new(env!("CARGO_MANIFEST_DIR")).join("../sign/tests/data/ec-p256.p12");
            std::fs::copy(&id, root.join("id.p12")).expect("copy the test digital ID");
            let doc = blank_doc(a);
            let args = json!({ "doc": doc, "id": "id.p12", "password": "test", "out": "signed.pdf" });
            args.as_object().cloned().unwrap_or_default()
        }
        _ => serde_json::Map::new(),
    }
}

#[test]
fn no_tool_panics_on_hostile_arguments() {
    let root = tmp("args");
    let mut a = Automation::new().with_root(&root).expect("root");
    let values = zoo();
    let mut failures = Vec::new();
    let mut calls = 0usize;

    for def in tools() {
        empty_dir(&root);
        // The whole arguments object, not just its properties.
        for bad in [json!(null), json!([]), json!("string"), json!(7), json!(true), json!({})] {
            calls += 1;
            if let Err(e) = guarded(&mut a, def.name, &bad) {
                failures.push(e);
            }
        }
        let props: Vec<String> = def.input_schema["properties"].as_object().map(|p| p.keys().cloned().collect()).unwrap_or_default();
        for prop in &props {
            for value in &values {
                let mut obj = plausible(&def);
                let fixed = fixture(&mut a, &root, def.name);
                let fixture_doc = fixed.get("doc").cloned();
                obj.extend(fixed);
                obj.insert(prop.clone(), value.clone());
                calls += 1;
                if let Err(e) = guarded(&mut a, def.name, &Value::Object(obj)) {
                    failures.push(e);
                }
                // Later tools pass placeholder document ids, which must not find this one open.
                if let Some(doc) = fixture_doc {
                    a.call("doc_close", &json!({ "doc": doc, "discard_changes": true })).expect("close the fixture's document");
                }
            }
        }
        // A signed file shows the fixture got calls past the ID and document checks.
        if def.name == "sign_document" {
            assert!(root.join("signed.pdf").is_file(), "no sign_document call signed: the hostile values stop at the ID or document");
        }
    }

    eprintln!("hostile arguments: {calls} calls over {} tools, {} panics", tools().len(), failures.len());
    for f in failures.iter().take(20) {
        eprintln!("  {f}");
    }
    let _ = std::fs::remove_dir_all(&root);
    assert!(failures.is_empty(), "{} tool call(s) panicked; the first is: {}", failures.len(), failures.first().map_or("", String::as_str));
}

#[test]
fn no_tool_escapes_the_root() {
    let base = tmp("escape");
    let root = base.join("root");
    std::fs::create_dir_all(&root).expect("root");
    // What a traversal would reach, and the state we require to be unchanged afterwards.
    let outside = base.join("outside.pdf");
    std::fs::write(
        &outside,
        b"%PDF-1.7
% not yours
",
    )
    .expect("outside file");
    let before = listing(&base);

    // A link inside the root pointing out of it, where the system allows one to be made. Without
    // it `link-out/…` is an ordinary path inside the root, which tools that write rightly accept.
    let linked = match dir_symlink(&base, &root.join("link-out")) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("escapes through a symbolic link not checked: {e}");
            false
        }
    };

    let mut a = Automation::new().with_root(&root).expect("root");

    // A real document, so tools reach their path arguments instead of failing on `doc` first.
    let doc = blank_doc(&mut a);

    let mut leaked = Vec::new();
    let mut tried = 0usize;
    let mut refused = 0usize;

    for def in tools() {
        for (prop, is_list) in path_props(&def) {
            for escape in escapes(&base, linked) {
                let mut obj = plausible_with(&def, Some(doc));
                obj.insert(prop.clone(), if is_list { json!([escape]) } else { json!(escape) });
                let args = Value::Object(obj);
                tried += 1;
                match catch_unwind(AssertUnwindSafe(|| a.call(def.name, &args))) {
                    // A path that leaves the root must be refused. Succeeding means the tool
                    // either used it or ignored an argument it declares — both are faults.
                    Ok(Ok(_)) => leaked.push(format!("{} accepted {prop}={escape:?}", def.name)),
                    Ok(Err(_)) => refused += 1,
                    Err(_) => leaked.push(format!("{} panicked on {prop}={escape:?}", def.name)),
                }
            }
        }
    }

    // Second net: nothing outside the root may have appeared, vanished or changed.
    let after = listing(&base);
    for (path, size) in &after {
        if path.starts_with(&root) {
            continue;
        }
        match before.iter().find(|(p, _)| p == path) {
            Some((_, was)) if was == size => {}
            Some(_) => leaked.push(format!("{} was modified outside the root", path.display())),
            None => leaked.push(format!("{} was created outside the root", path.display())),
        }
    }
    for (path, _) in &before {
        if !path.starts_with(&root) && !after.iter().any(|(p, _)| p == path) {
            leaked.push(format!("{} was deleted outside the root", path.display()));
        }
    }

    eprintln!("root confinement: {tried} escape attempts over {} tools, {refused} refused, {} leaks", tools().len(), leaked.len());
    for l in leaked.iter().take(20) {
        eprintln!("  {l}");
    }
    let _ = std::fs::remove_dir_all(&base);
    assert!(tried > 100, "only {tried} escape attempts: the path properties are not being found");
    assert!(leaked.is_empty(), "{} confinement failure(s); the first is: {}", leaked.len(), leaked.first().map_or("", String::as_str));
}

/// Every regular file under `dir` with its size, not following symbolic links.
fn listing(dir: &Path) -> Vec<(PathBuf, u64)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
            if meta.file_type().is_symlink() {
                continue; // do not walk through a link we planted
            }
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() {
                out.push((path, meta.len()));
            }
        }
    }
    out.sort();
    out
}

/// A six-page file whose fourth `/Kids` entry points at an object that does not exist (#2660),
/// with a link, a URI link and a comment on every page, and a form field and text added by
/// PdfCraft (its `/PCAdded` marker) on the last. Readers
/// disagree on such a file: the viewer stopped at the dangling entry and saw three pages, while
/// the editing layer sees all six, so annotations come back on pages the viewer does not have.
fn dangling_kid_pdf() -> Vec<u8> {
    let pages: Vec<u32> = (0..6).map(|i| 10 + 2 * i).collect();
    let mut kids: Vec<u32> = pages[..3].to_vec();
    kids.push(77); // does not exist
    kids.extend(&pages[3..]);
    let kids: Vec<String> = kids.iter().map(|k| format!("{k} 0 R")).collect();
    let mut objs: Vec<(u32, String)> = vec![
        (1, "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [50 0 R] >> >>".into()),
        (2, format!("<< /Type /Pages /Kids [{}] /Count 6 >>", kids.join(" "))),
        (90, "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into()),
    ];
    for (i, &page) in pages.iter().enumerate() {
        let text = format!("BT /F1 24 Tf 50 300 Td (Page {}) Tj ET", i + 1);
        let (field, contents) = if i == 5 { (" 50 0 R", format!("[{} 0 R 60 0 R]", page + 1)) } else { ("", format!("{} 0 R", page + 1)) };
        objs.push((
            page,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] /Resources << /Font << /F1 90 0 R >> >> /Contents {contents} \
                 /Annots [<< /Type /Annot /Subtype /Link /Rect [10 10 100 40] /Dest [10 0 R /Fit] >> \
                 << /Type /Annot /Subtype /Link /Rect [10 50 100 80] /A << /S /URI /URI (https://example.com/) >> >> \
                 << /Type /Annot /Subtype /Text /Rect [200 200 220 220] /Contents (note) /T (author) >>{field}] >>"
            ),
        ));
        objs.push((page + 1, format!("<< /Length {} >>\nstream\n{text}\nendstream", text.len())));
    }
    objs.push((50, "<< /FT /Tx /T (name) /Type /Annot /Subtype /Widget /Rect [50 100 250 130] /P 20 0 R >>".into()));
    let added = "BT /F1 12 Tf 72 80 Td (AddedSample) Tj ET";
    objs.push((
        60,
        format!(
            "<< /PCMark /Added /PCAdded << /Kind /Text /Rect [70 70 150 100] /Text (AddedSample) /Size 12 >> /Length {} >>\nstream\n{added}\nendstream",
            added.len()
        ),
    ));
    objs.sort_by_key(|(n, _)| *n);

    let size = 100;
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = vec![None; size];
    for (num, body) in &objs {
        if let Some(slot) = offsets.get_mut(*num as usize) {
            *slot = Some(out.len());
        }
        out.extend_from_slice(format!("{num} 0 obj\n{body}\nendobj\n").as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for offset in offsets.iter().skip(1) {
        out.extend_from_slice(
            match offset {
                Some(o) => format!("{o:010} 00000 n \n"),
                None => "0000000000 65535 f \n".to_string(),
            }
            .as_bytes(),
        );
    }
    out.extend_from_slice(format!("trailer << /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    out
}

#[test]
fn no_tool_panics_when_the_viewer_sees_fewer_pages_than_the_file_has() {
    // #2660: `link_list` indexed the viewer's pages with a page number from the editing layer
    // ("index out of bounds: the len is 3 but the index is 3"). Every tool, on a fresh copy of a
    // file where the two disagree, must answer with a result or an error.
    let root = tmp("dangling-kid");
    // The JSON a call answers with, or the error it gave.
    let answer = |a: &mut Automation, tool: &str, args: &Value| -> std::result::Result<Value, String> {
        match catch_unwind(AssertUnwindSafe(|| a.call(tool, args))) {
            Ok(Ok(content)) => content
                .into_iter()
                .find_map(|c| match c {
                    pdfcraft_automation::Content::Json(v) => Some(v),
                    pdfcraft_automation::Content::Png { .. } => None,
                })
                .ok_or_else(|| format!("{tool} answered without JSON")),
            Ok(Err(e)) => Err(format!("{tool} failed: {e:?}")),
            Err(_) => Err(format!("{tool} panicked")),
        }
    };
    let pdf = dangling_kid_pdf();
    let mut failures = Vec::new();
    for def in tools() {
        // Written again for every tool: tools that save write to `in.pdf`, the plausible path.
        std::fs::write(root.join("in.pdf"), &pdf).expect("fixture");
        let mut a = Automation::new().with_root(&root).expect("root");
        let opened = a.call("doc_open", &json!({ "path": "in.pdf" })).expect("open the damaged file");
        let doc = opened
            .into_iter()
            .find_map(|c| match c {
                pdfcraft_automation::Content::Json(v) => v["doc"].as_u64(),
                pdfcraft_automation::Content::Png { .. } => None,
            })
            .expect("the opened document has an id");
        if let Err(e) = guarded(&mut a, def.name, &Value::Object(plausible_with(&def, Some(doc)))) {
            failures.push(e);
        }
        match def.name {
            // The field sits on the sixth page; a rect is placed on the field's own page. That is
            // refused, saying why, and the field is left as it was.
            "form_set_props" => {
                let fields = |a: &mut Automation| answer(a, "form_fields", &json!({ "doc": doc }));
                let before = fields(&mut a);
                match catch_unwind(AssertUnwindSafe(|| a.call(def.name, &json!({ "doc": doc, "field": "name", "rect": [10, 10, 60, 30] })))) {
                    Ok(Err(ToolError::Failed(m))) if m == "name is on page 6, which could not be read" => {}
                    Ok(Err(e)) => failures.push(format!("form_set_props gave the wrong error: {e:?}")),
                    Ok(Ok(_)) => failures.push("form_set_props placed a rect on a page the viewer could not read".into()),
                    Err(_) => failures.push("form_set_props panicked on a rect for the field on page 6".into()),
                }
                let after = fields(&mut a);
                if before.is_err() || before != after {
                    failures.push(format!("form_set_props changed the field: {before:?} became {after:?}"));
                }
            }
            // Listing still reports what is on the pages the viewer lacks, not just survives it.
            "link_list" => match answer(&mut a, def.name, &json!({ "doc": doc })) {
                Ok(v) if v["count"] == 12 => {}
                other => failures.push(format!("link_list did not list the file's 12 links: {other:?}")),
            },
            // The added text on page 6 is reported, its rect unchanged (user space), as
            // `doc_info` reports rects on pages the viewer could not read.
            "content_list" => match answer(&mut a, def.name, &json!({ "doc": doc })) {
                Ok(v)
                    if v["items"].as_array().is_some_and(|items| {
                        items.iter().any(|it| it["page"] == 6 && it["type"] == "text" && it["rect"] == json!([70.0, 70.0, 150.0, 100.0]))
                    }) => {}
                other => failures.push(format!("content_list did not report the added text on page 6 at [70, 70, 150, 100]: {other:?}")),
            },
            _ => {}
        }
    }
    for f in &failures {
        eprintln!("  {f}");
    }
    let _ = std::fs::remove_dir_all(&root);
    assert!(failures.is_empty(), "{} failure(s); the first is: {}", failures.len(), failures.first().map_or("", String::as_str));
}
