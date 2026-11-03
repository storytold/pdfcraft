//! The AI assistant's tools end to end (M13.A5): the library kept in a file inside the root, and
//! `ai_ask` returning the request it would send, on synthetic PDFs. Nothing here touches a
//! network: the build has no transport.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

/// One 200 × 300 pt page per line of `lines`; `redact` puts an unapplied redaction mark over the
/// second line of the first page.
fn pdf(pages: &[&[&str]], redact: bool) -> Vec<u8> {
    let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into()];
    let kids: Vec<String> = (0..pages.len()).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {} /MediaBox [0 0 200 300] >>", kids.join(" "), pages.len()));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
    let annot = 4 + 2 * pages.len();
    for (i, lines) in pages.iter().enumerate() {
        let annots = if redact && i == 0 { format!(" /Annots [{annot} 0 R]") } else { String::new() };
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >>{annots} >>", 5 + 2 * i));
        let body: String = lines.iter().enumerate().map(|(n, l)| format!("BT /F1 12 Tf 20 {} Td ({l}) Tj ET ", 250 - 100 * n)).collect();
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
    }
    objs.push("<< /Type /Annot /Subtype /Redact /Rect [10 140 190 170] >>".into());
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn birds() -> Vec<u8> {
    pdf(&[&["Kestrels hover over fields."], &["Falcons stoop at great speed."]], false)
}

fn report() -> Vec<u8> {
    pdf(&[&["Rates rose three percent.", "Secret salary 90000."]], true)
}

#[track_caller]
fn workdir(test: &str) -> PathBuf {
    let caller = std::panic::Location::caller();
    let dir = std::env::temp_dir().join(format!("pdfcraft-ai-{test}-{}-{}", std::process::id(), caller.line()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("root")).unwrap();
    std::fs::write(dir.join("root/birds.pdf"), birds()).unwrap();
    std::fs::write(dir.join("root/report.pdf"), report()).unwrap();
    dir
}

fn auto(dir: &Path) -> Automation {
    Automation::new().with_root(dir.join("root")).unwrap().with_clock(|| 1_700_000_000)
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

#[test]
fn the_library_lives_in_its_file_and_knows_documents_by_their_bytes() {
    let dir = workdir("library");
    let mut a = auto(&dir);
    let added = ok(&mut a, "ai_library_add", json!({"paths": ["birds.pdf", "report.pdf", "missing.pdf"], "library": "lib.json"}));
    let r = &added["results"];
    assert_eq!((r[0]["outcome"].as_str(), r[0]["status"].as_str(), r[0]["passages"].as_u64()), (Some("added"), Some("indexed"), Some(2)), "{added}");
    assert_eq!(r[1]["outcome"], "added");
    assert!(r[2]["error"].is_string(), "a missing file is reported, the rest are added: {added}");
    assert!(dir.join("root/lib.json").is_file());

    // A new session reads the same library back from the file.
    let mut b = auto(&dir);
    let listed = ok(&mut b, "ai_library_list", json!({"library": "lib.json"}));
    assert_eq!(listed["documents"].as_array().map(Vec::len), Some(2), "{listed}");
    let hits = ok(&mut b, "ai_library_search", json!({"query": "falcons speed", "k": 3}));
    assert_eq!((hits["results"][0]["title"].as_str(), hits["results"][0]["page"].as_u64()), (Some("birds.pdf"), Some(2)), "{hits}");

    // The same bytes under another name are the same document, now with two known paths.
    std::fs::create_dir_all(dir.join("root/moved")).unwrap();
    std::fs::write(dir.join("root/moved/renamed.pdf"), birds()).unwrap();
    let again = ok(&mut b, "ai_library_add", json!({"paths": ["moved/renamed.pdf"]}));
    assert_eq!(again["results"][0]["outcome"], "already_in_library");
    let listed = ok(&mut b, "ai_library_list", json!({}));
    let bird = listed["documents"].as_array().unwrap().iter().find(|d| d["title"] == "birds.pdf").unwrap().clone();
    assert_eq!(bird["paths"].as_array().map(Vec::len), Some(2));
    assert!(bird["paths"][0].as_str().unwrap().ends_with("renamed.pdf"));

    let removed = ok(&mut b, "ai_library_remove", json!({"sha256": bird["sha256"]}));
    assert_eq!(removed["removed"], true);
    assert_eq!(ok(&mut b, "ai_library_remove", json!({"sha256": bird["sha256"]}))["removed"], false);
    let mut c = auto(&dir);
    assert_eq!(ok(&mut c, "ai_library_list", json!({"library": "lib.json"}))["documents"].as_array().map(Vec::len), Some(1));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_unreadable_library_file_is_refused_and_left_alone() {
    let dir = workdir("garbage");
    std::fs::write(dir.join("root/lib.json"), b"{not a library").unwrap();
    let mut a = auto(&dir);
    let e = a.call("ai_library_add", &json!({"paths": ["birds.pdf"], "library": "lib.json"})).unwrap_err();
    assert!(e.to_string().contains("not changed"), "{e}");
    assert_eq!(std::fs::read(dir.join("root/lib.json")).unwrap(), b"{not a library");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ai_ask_returns_the_request_and_sends_nothing() {
    let dir = workdir("ask");
    let mut a = auto(&dir);
    ok(&mut a, "ai_library_add", json!({"paths": ["birds.pdf", "report.pdf"], "library": "lib.json"}));
    let doc = ok(&mut a, "doc_open", json!({"path": "report.pdf"}))["doc"].clone();

    let refused = a.call("ai_ask", &json!({"doc": doc, "page": 1, "question": "Why did rates rise?", "dry_run": false})).unwrap_err();
    assert!(refused.to_string().contains("dry_run"), "{refused}");

    let asked = ok(&mut a, "ai_ask", json!({"doc": doc, "page": 1, "question": "Why did rates rise? Compare kestrels.", "dry_run": true}));
    assert_eq!(asked["sent"], false);
    let request = &asked["request"];
    assert_eq!(request["url"], "https://api.anthropic.com/v1/messages");
    assert!(request["headers"].as_array().unwrap().iter().any(|h| h[0] == "x-api-key" && h[1] == "••••"), "{request}");
    let body = &request["body"];
    assert_eq!(body["model"], "claude-opus-5-5");
    let content = body["messages"][0]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "search_result");
    assert_eq!(content[0]["source"], "pdfcraft-doc:i0");
    assert_eq!(content.last().unwrap()["text"], "Why did rates rise? Compare kestrels.");
    // The other library document is cited; the document being read isn't repeated from the library.
    let sources: Vec<&Value> = asked["passages"].as_array().unwrap().iter().map(|p| &p["source"]).collect();
    assert!(sources.iter().any(|s| s["kind"] == "library"), "{sources:?}");
    let report_hash = ok(&mut a, "ai_library_list", json!({}))["documents"].as_array().unwrap().iter().find(|d| d["title"] == "report.pdf").unwrap()
        ["sha256"]
        .clone();
    assert!(sources.iter().all(|s| s["sha256"] != report_hash), "{sources:?}");

    // Text under the unapplied redaction mark is left out, and counted.
    let sent = body.to_string();
    assert!(sent.contains("Rates rose three percent.") && !sent.contains("Secret") && !sent.contains("90000"), "{sent}");
    assert!(asked["redacted_glyphs_left_out"].as_u64().unwrap() > 0);

    // A selection (top line only), no library passages, another model.
    let narrow = ok(
        &mut a,
        "ai_ask",
        json!({"doc": doc, "page": 1, "question": "Explain", "rect": [0, 30, 200, 70], "k": 0, "model": "claude-haiku-5-5", "dry_run": true}),
    );
    assert_eq!(narrow["request"]["body"]["model"], "claude-haiku-5-5");
    assert!(narrow["passages"].as_array().unwrap().iter().all(|p| p["source"]["kind"] == "this_document"));
    assert!(narrow["request"]["body"].get("fallbacks").is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn copy_forbidden_documents_are_refused_and_never_indexed() {
    let dir = workdir("forbidden");
    let mut cos = pdfcraft_cos::Document::open(Arc::new(birds())).unwrap();
    cos.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: "",
        owner_password: "owner",
        permissions: -1 ^ 0b1_0000,
        encrypt_metadata: true,
        seed: [7; 32],
    })
    .unwrap();
    std::fs::write(dir.join("root/locked.pdf"), pdfcraft_cos::write_full(&cos, &Default::default()).unwrap()).unwrap();
    let mut a = auto(&dir);
    let added = ok(&mut a, "ai_library_add", json!({"paths": ["locked.pdf"]}));
    assert_eq!((added["results"][0]["status"].as_str(), added["results"][0]["passages"].as_u64()), (Some("not_permitted"), Some(0)));
    let doc = ok(&mut a, "doc_open", json!({"path": "locked.pdf"}))["doc"].clone();
    let e = a.call("ai_ask", &json!({"doc": doc, "page": 1, "question": "What is this?", "dry_run": true})).unwrap_err();
    assert!(e.to_string().contains("copying"), "{e}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The paths these tools read and write stay inside the root. The files outside exist, so a
/// refusal is the confinement and not a missing file.
#[test]
fn library_paths_stay_inside_the_root() {
    let dir = workdir("confined");
    std::fs::write(dir.join("outside.pdf"), birds()).unwrap();
    std::fs::write(dir.join("outside.json"), b"{}").unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({"path": "birds.pdf"}))["doc"].clone();
    assert!(a.call("ai_library_add", &json!({"paths": ["birds.pdf", "../outside.pdf"]})).is_err(), "one path outside refuses the call");
    assert_eq!(ok(&mut a, "ai_library_list", json!({}))["documents"].as_array().map(Vec::len), Some(0), "and nothing was added");
    for (tool, args) in [
        ("ai_library_add", json!({"paths": ["birds.pdf"], "library": "../outside.json"})),
        ("ai_library_list", json!({"library": "../outside.json"})),
        ("ai_library_search", json!({"query": "kestrel", "library": "../outside.json"})),
        ("ai_library_remove", json!({"sha256": "0".repeat(64), "library": "../outside.json"})),
        ("ai_ask", json!({"doc": doc, "page": 1, "question": "q", "dry_run": true, "library": "../outside.json"})),
    ] {
        assert!(a.call(tool, &args).is_err(), "{tool} {args}");
    }
    assert_eq!(std::fs::read(dir.join("outside.json")).unwrap(), b"{}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A library file that exists but can't be read (here a folder, which fails on every system) is
/// refused, and nothing is written in its place.
#[test]
fn a_library_that_cant_be_read_is_refused() {
    let dir = workdir("unreadable");
    std::fs::create_dir_all(dir.join("root/lib.json")).unwrap();
    let mut a = auto(&dir);
    assert!(a.call("ai_library_add", &json!({"paths": ["birds.pdf"], "library": "lib.json"})).is_err());
    assert!(a.call("ai_library_list", &json!({"library": "lib.json"})).is_err());
    assert!(dir.join("root/lib.json").is_dir());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Two sessions on one library file: each change is made to what the file holds now, so neither
/// writes over what the other saved.
#[test]
fn two_sessions_on_one_library_keep_each_others_additions() {
    let dir = workdir("two-sessions");
    let mut a = auto(&dir);
    let mut b = auto(&dir);
    ok(&mut a, "ai_library_add", json!({"paths": ["birds.pdf"], "library": "lib.json"}));
    assert_eq!(ok(&mut b, "ai_library_list", json!({"library": "lib.json"}))["documents"].as_array().map(Vec::len), Some(1));
    ok(&mut a, "ai_library_add", json!({"paths": ["report.pdf"]}));
    std::fs::create_dir_all(dir.join("root/moved")).unwrap();
    std::fs::write(dir.join("root/moved/renamed.pdf"), birds()).unwrap();
    ok(&mut b, "ai_library_add", json!({"paths": ["moved/renamed.pdf"]}));
    let mut c = auto(&dir);
    let titles: Vec<String> = ok(&mut c, "ai_library_list", json!({"library": "lib.json"}))["documents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["title"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(titles.len(), 2, "b kept a's report.pdf: {titles:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// With separate read and write folders, a library in the write folder alone can't be read
/// (its titles, paths and text would come back in the results), while one in the read folder can.
#[test]
fn a_library_is_read_only_from_the_read_folder() {
    use pdfcraft_automation::file_access::AutomationRoots;
    let dir = workdir("roots");
    for d in ["in", "out"] {
        std::fs::create_dir_all(dir.join(d)).unwrap();
    }
    std::fs::write(dir.join("in/birds.pdf"), birds()).unwrap();
    // A library with a document in it, in each folder.
    let mut plain = Automation::new().with_root(&dir).unwrap();
    for f in ["in/lib.json", "out/lib.json"] {
        ok(&mut plain, "ai_library_add", json!({"paths": ["in/birds.pdf"], "library": f}));
    }
    let s = |p: &str| dir.join(p).to_string_lossy().into_owned();
    let roots = AutomationRoots::new(Some(dir.join("in").as_path()), Some(dir.join("out").as_path())).unwrap();
    let mut a = Automation::new().with_automation_roots(roots);
    for (tool, args) in [
        ("ai_library_list", json!({"library": s("out/lib.json")})),
        ("ai_library_search", json!({"query": "kestrels", "library": s("out/lib.json")})),
        ("ai_library_add", json!({"paths": [s("in/birds.pdf")], "library": s("out/lib.json")})),
    ] {
        let e = a.call(tool, &args);
        assert!(e.is_err(), "{tool} read a library outside the read folder: {e:?}");
    }
    assert_eq!(ok(&mut a, "ai_library_list", json!({"library": s("in/lib.json")}))["documents"].as_array().map(Vec::len), Some(1));
    // Named by a tool that only reads, the read folder's library still can't be written.
    assert!(a.call("ai_library_add", &json!({"paths": [s("in/birds.pdf")]})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A copy of the open file indexed before it was marked for redaction is not searched for a
/// question about it: it still holds the text the mark hides.
#[test]
fn ai_ask_leaves_out_an_earlier_copy_of_the_open_file() {
    let dir = workdir("stale");
    let unmarked = pdf(&[&["Rates rose three percent.", "Secret salary 90000."]], false);
    std::fs::write(dir.join("root/report.pdf"), unmarked).unwrap();
    let mut a = auto(&dir);
    ok(&mut a, "ai_library_add", json!({"paths": ["report.pdf", "birds.pdf"], "library": "lib.json"}));
    // The file is marked for redaction since, and opened.
    std::fs::write(dir.join("root/report.pdf"), report()).unwrap();
    let doc = ok(&mut a, "doc_open", json!({"path": "report.pdf"}))["doc"].clone();
    let asked = ok(&mut a, "ai_ask", json!({"doc": doc, "page": 1, "question": "What is the secret salary?", "dry_run": true}));
    let sent = asked["request"]["body"].to_string();
    assert!(!sent.contains("Secret") && !sent.contains("90000"), "{sent}");
    let _ = std::fs::remove_dir_all(&dir);
}
