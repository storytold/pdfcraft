//! Independent bounded multi-sheet DXGI contracts. Ordinary tests request only invalid
//! native destinations. The sole real job is ignored, Windows-only and requires its own
//! fresh, explicit local evidence root; a backend error or missing opt-in never counts as PASS.
use super::*;

const PRINTER: &str = "Microsoft Print to PDF";
const NFD: [&str; 2] = ["Birinci sayfa: C\u{327}ag\u{306}rı I\u{307}stanbul", "I\u{307}kinci sayfa: O\u{308}g\u{306}renci S\u{327}is\u{327}li"];
const MARKS: [&str; 2] = ["111", "222"];
type PagePixels = (u32, u32, pdfcraft_render::Pixels);

struct SourceSnapshot {
    bytes: Arc<Vec<u8>>,
    state: Value,
    texts: [String; 2],
    pixels: [PagePixels; 2],
}

fn text(a: &mut Automation, id: DocId, page: usize) -> String {
    // Preserve the exact stored codepoints and internal whitespace, including all NFD marks.
    json(a, "text_extract", json!({"doc":id.0,"pages":[page]}))["pages"][0]["text"].as_str().unwrap().trim_matches(['\r', '\n']).to_owned()
}

fn source(root: &Path, pages: usize) -> (Automation, DocId) {
    assert!(pages >= 2);
    let mut a = Automation::new().with_root(root).unwrap().with_clock(|| 1_700_000_000);
    let id = DocId(json(&mut a, "doc_create", json!({"from":"blank","pages":pages,"width":595.28,"height":841.89}))["doc"].as_u64().unwrap());
    // Four authoring commands, in a known order, let the tests drain and replay the complete
    // undo history afterwards. Text and marks use only the project's embedded, allowed faces.
    for (page, original) in NFD.iter().enumerate() {
        let result = json(&mut a, "page_add_text", json!({"doc":id.0,"page":page+1,"text":original,"at":[40,60],"width":510,"size":18}));
        assert!(result["warnings"].as_array().is_none_or(Vec::is_empty), "{result}");
    }
    for (page, at) in [[60, 210], [380, 510]].iter().enumerate() {
        let result = json(&mut a, "page_add_text", json!({"doc":id.0,"page":page+1,"text":MARKS[page],"at":at,"width":180,"size":64}));
        assert!(result["warnings"].as_array().is_none_or(Vec::is_empty), "{result}");
    }
    (a, id)
}

fn ink(pixels: &PagePixels, rect: [u32; 4]) -> usize {
    let width = pixels.0 as usize;
    assert!(width > 0);
    pixels
        .2
        .chunks_exact(4)
        .enumerate()
        .filter(|(i, p)| {
            let (x, y) = ((i % width) as u32, (i / width) as u32);
            x >= rect[0] && x < rect[2] && y >= rect[1] && y < rect[3] && p[3] > 0 && p[0] < 128 && p[1] < 128 && p[2] < 128
        })
        .count()
}

fn distinct_ink(pages: &[PagePixels; 2]) -> [[usize; 3]; 2] {
    let counts = pages.each_ref().map(|p| [ink(p, [30, 45, 570, 130]), ink(p, [40, 185, 260, 330]), ink(p, [350, 485, 580, 630])]);
    for (page, c) in counts.iter().enumerate() {
        assert!(c[0] > 40, "page {} must draw text ink above its separate marker: {counts:?}", page + 1);
    }
    assert!(counts[0][1] > 100 && counts[0][2] < 10, "page 1 must retain only its upper-left marker: {counts:?}");
    assert!(counts[1][2] > 100 && counts[1][1] < 10, "page 2 must retain only its lower-right marker: {counts:?}");
    assert!(pages[0] != pages[1], "a repeated page cannot stand in for two distinct source sheets");
    counts
}

fn snapshot(a: &mut Automation, id: DocId) -> SourceSnapshot {
    let texts = [text(a, id, 1), text(a, id, 2)];
    for (page, original) in texts.iter().enumerate() {
        assert!(original.contains(NFD[page]), "source page {} must keep exact NFD: {original:?}", page + 1);
        assert!(original.contains(MARKS[page]));
    }
    let pixels = [a.render_pixels(id.0, 1, 1.0).unwrap(), a.render_pixels(id.0, 2, 1.0).unwrap()];
    distinct_ink(&pixels);
    let state = json(a, "doc_info", json!({"doc":id.0}))["document"].clone();
    assert_eq!(state["dirty"], true, "the native operation must preserve real unsaved authoring work");
    assert!(state["undo"].as_str().is_some(), "undo exposes the pending authoring command's label");
    assert!(state["redo"].is_null(), "there must be no redo command before the test");
    SourceSnapshot { bytes: a.session.save_bytes(id).unwrap(), state, texts, pixels }
}

fn assert_unchanged(a: &mut Automation, id: DocId, before: &SourceSnapshot) {
    assert_eq!(a.session.save_bytes(id).unwrap(), before.bytes, "printing/rejection must not rewrite source bytes");
    assert_eq!([text(a, id, 1), text(a, id, 2)], before.texts, "printing/rejection must not normalize Unicode or change source text");
    assert!(a.render_pixels(id.0, 1, 1.0).unwrap() == before.pixels[0]);
    assert!(a.render_pixels(id.0, 2, 1.0).unwrap() == before.pixels[1]);
    assert_eq!(json(a, "doc_info", json!({"doc":id.0}))["document"], before.state, "path/dirty/undo/redo state must remain unchanged");
}

fn assert_authoring_history(a: &mut Automation, id: DocId, before: &SourceSnapshot) {
    // Undo only the four fixture edits. Native calls must not append even a hidden no-op.
    json(a, "edit_undo", json!({"doc":id.0}));
    assert!(!text(a, id, 2).contains(MARKS[1]));
    assert!(text(a, id, 2).contains(NFD[1]));
    assert!(text(a, id, 1).contains(MARKS[0]));
    json(a, "edit_undo", json!({"doc":id.0}));
    assert_eq!(text(a, id, 1), NFD[0]);
    json(a, "edit_undo", json!({"doc":id.0}));
    assert_eq!(text(a, id, 2), "");
    json(a, "edit_undo", json!({"doc":id.0}));
    assert_eq!(text(a, id, 1), "");
    assert!(a.call("edit_undo", &json!({"doc":id.0})).is_err(), "no print operation may enter the authoring undo history");
    for _ in 0..4 {
        json(a, "edit_redo", json!({"doc":id.0}));
    }
    assert!(a.call("edit_redo", &json!({"doc":id.0})).is_err());
    assert_eq!([text(a, id, 1), text(a, id, 2)], before.texts);
    assert!(a.render_pixels(id.0, 1, 1.0).unwrap() == before.pixels[0]);
    assert!(a.render_pixels(id.0, 2, 1.0).unwrap() == before.pixels[1]);
    assert_eq!(json(a, "doc_info", json!({"doc":id.0}))["document"], before.state);
}

fn args(id: DocId, output: &str) -> Value {
    json!({"doc":id.0,"printer":PRINTER,"printer_output":output,"as_image":false,
        "pages":"1-2","paper":"A4","orientation":"portrait","layout":"actual",
        "comments_forms":"document","copies":1,"duplex":"off","options":{}})
}

#[test]
fn gdi_still_rejects_two_imposed_sheets_without_any_job() {
    let dir = tempfile::tempdir().unwrap();
    let (mut a, id) = source(dir.path(), 2);
    let before = snapshot(&mut a, id);
    let mut request = args(id, "never-gdi.pdf");
    request["as_image"] = json!(true);
    let error = a.call("doc_print", &request).unwrap_err();
    assert!(matches!(error, ToolError::InvalidArgs(_)), "GDI sheet-limit validation must precede all native calls: {error}");
    assert!(error.to_string().contains("sheet"), "{error}");
    assert_unchanged(&mut a, id, &before);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "no final or staging files may exist");
    assert_authoring_history(&mut a, id, &before);
}

#[test]
fn dxgi_rejects_sixty_five_imposed_sheets_without_any_job() {
    let dir = tempfile::tempdir().unwrap();
    let (mut a, id) = source(dir.path(), 65);
    let before = snapshot(&mut a, id);
    let mut request = args(id, "never-65-sheets.pdf");
    request["pages"] = json!("1-65");
    // Actual layout puts each source page on its own sheet: this is 65 imposed sheets,
    // never a valid new 1..=64 request hidden in an ordinary test.
    let error = a.call("doc_print", &request).unwrap_err();
    assert!(matches!(error, ToolError::InvalidArgs(_)), "the DXGI bound must reject before native submission: {error}");
    assert!(error.to_string().contains("sheet") || error.to_string().contains("64"), "{error}");
    assert_unchanged(&mut a, id, &before);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    assert_authoring_history(&mut a, id, &before);
}

#[test]
fn multipage_native_arguments_reject_before_native_work_and_preserve_history() {
    let dir = tempfile::tempdir().unwrap();
    let (mut a, id) = source(dir.path(), 2);
    let before = snapshot(&mut a, id);
    let base = args(id, "never-invalid.pdf");
    for (key, value) in [
        ("printer", json!("default")),
        ("printer", json!("Office")),
        ("printer", json!("\\\\server\\Microsoft Print to PDF")),
        ("copies", json!(2)),
        ("duplex", json!("long-edge")),
        ("duplex", json!("printer")),
        ("options", json!({"media":"A4"})),
        ("path", json!("ordinary-export.pdf")),
        ("printer_output", json!("")),
        ("as_image", Value::Null),
    ] {
        let mut request = base.clone();
        request[key] = value;
        let result = a.call("doc_print", &request);
        assert!(matches!(result, Err(ToolError::InvalidArgs(_))), "invalid native arguments must be pure rejection: {request}: {result:?}");
        assert_unchanged(&mut a, id, &before);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
    assert_authoring_history(&mut a, id, &before);
}

#[cfg(windows)]
#[test]
fn native_destination_confinement_and_existing_files_reject_before_any_job() {
    let outer = tempfile::tempdir().unwrap();
    let allowed = outer.path().join("allowed");
    std::fs::create_dir(&allowed).unwrap();
    let (mut a, id) = source(&allowed, 2);
    let before = snapshot(&mut a, id);
    let outside = outer.path().join("outside.pdf");
    for path in ["../outside.pdf".to_string(), outside.to_string_lossy().into_owned(), "not-a-pdf.txt".to_string()] {
        let mut request = args(id, &path);
        // One sheet isolates the destination preflight in both the old and new implementation;
        // the second source page and its Unicode remain part of the protected source snapshot.
        request["pages"] = json!("1");
        let error = a.call("doc_print", &request).unwrap_err().to_string();
        assert!(error.contains("outside") || error.contains("root") || error.contains(".pdf"), "{error}");
        assert!(!outside.exists());
        assert_eq!(std::fs::read_dir(&allowed).unwrap().count(), 0);
        assert_unchanged(&mut a, id, &before);
    }
    let target = allowed.join("owned.pdf");
    std::fs::write(&target, b"pre-existing owner bytes").unwrap();
    let mut request = args(id, "owned.pdf");
    request["pages"] = json!("1");
    let error = a.call("doc_print", &request).unwrap_err().to_string();
    assert!(error.contains("exists") || error.contains("replace"), "{error}");
    assert_eq!(std::fs::read(&target).unwrap(), b"pre-existing owner bytes");
    assert_eq!(std::fs::read_dir(&allowed).unwrap().count(), 1, "rejection must leave no staging residue");
    assert_unchanged(&mut a, id, &before);
    assert_authoring_history(&mut a, id, &before);
}

#[cfg(windows)]
fn explicit_native_root() -> PathBuf {
    let value = std::env::var_os("PDFCRAFT_REAL_DXGI_MULTIPAGE_ROOT")
        .expect("explicit multipage native test requires PDFCRAFT_REAL_DXGI_MULTIPAGE_ROOT; missing opt-in must fail, never skip as PASS");
    let root = PathBuf::from(value);
    let local = |path: &Path| {
        matches!(path.components().next(), Some(std::path::Component::Prefix(p))
        if matches!(p.kind(), std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_)))
    };
    assert!(root.is_absolute() && local(&root), "native root must be an absolute local-drive directory");
    let root = root.canonicalize().expect("caller must prepare a fresh native evidence root");
    assert!(root.is_dir() && local(&root), "canonical native evidence root must stay local");
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0, "use a new empty evidence directory; tests never remove old results");
    root
}

#[cfg(windows)]
fn completed_two_page_pdf(path: &Path) -> Arc<Vec<u8>> {
    use std::io::Read;
    const LIMIT: u64 = 128 * 1024 * 1024;
    let file = std::fs::File::open(path).expect("DXGI success must publish one real completed PDF");
    let metadata = file.metadata().unwrap();
    assert!(metadata.is_file() && metadata.len() <= LIMIT);
    let mut bytes = Vec::new();
    file.take(LIMIT + 1).read_to_end(&mut bytes).unwrap();
    assert!(bytes.len() as u64 <= LIMIT && bytes.starts_with(b"%PDF-"));
    let end = bytes.iter().rposition(|b| !b.is_ascii_whitespace()).map_or(0, |i| i + 1);
    assert!(bytes.get(..end).is_some_and(|b| b.ends_with(b"%%EOF")), "native PDF must have a completed EOF marker");
    let bytes = Arc::new(bytes);
    let cos = pdfcraft_cos::Document::open(bytes.clone()).unwrap();
    assert!(cos.repair_log().is_empty(), "native output needs repair: {:?}", cos.repair_log());
    assert_eq!(pdfcraft_model::pages(&cos).len(), 2, "one native output file must contain both imposed sheets");
    pdfcraft_render::check_pages(bytes.clone(), None, 2).unwrap();
    let info = pdfcraft_render::inspect(bytes.clone(), None).unwrap();
    assert_eq!(info.pages.len(), 2);
    for page in &info.pages {
        // Permit driver point rounding; Letter or landscape cannot satisfy these A4 bounds.
        assert!((page.width - 595.28).abs() <= 1.0 && (page.height - 841.89).abs() <= 1.0, "native page must be portrait A4: {page:?}");
    }
    bytes
}

#[cfg(windows)]
#[test]
#[ignore = "sends one explicit two-sheet Microsoft Print to PDF DXGI job; requires a fresh local PDFCRAFT_REAL_DXGI_MULTIPAGE_ROOT"]
fn windows_dxgi_multipage_writes_two_distinct_a4_pages_without_fallback_and_preserves_source() {
    use sha2::{Digest, Sha256};
    use std::io::Write;

    let root = explicit_native_root();
    let allowed = root.join("allowed");
    std::fs::create_dir(&allowed).unwrap();
    let (mut a, id) = source(&allowed, 2);
    let before = snapshot(&mut a, id);
    // Persist only our synthetic source outside the automation write root, so the allowed
    // directory can prove that exactly one native output and no staging artifacts remain.
    let source_path = root.join("synthetic-two-page-source.pdf");
    std::fs::OpenOptions::new().write(true).create_new(true).open(&source_path).unwrap().write_all(before.bytes.as_slice()).unwrap();
    let source_hash = Sha256::digest(before.bytes.as_slice());
    let source_hex: String = source_hash.iter().map(|b| format!("{b:02x}")).collect();
    eprintln!("multipage synthetic source: {} bytes, SHA-256 {source_hex}; exact NFD texts: {:?}", before.bytes.len(), before.texts);
    let request = args(id, "native-dxgi-two-pages.pdf");
    let result = json(&mut a, "doc_print", request.clone());
    assert_eq!(result["backend"], "windows-dxgi", "GDI or a missing backend must never pass the DXGI multipage regression: {result}");
    assert_eq!(result["as_image"], false);
    assert_eq!(result["sheets"], 2);
    assert_eq!(result["pages"], 2);
    eprintln!("explicit two-sheet DXGI response: {result}");
    let target = allowed.join("native-dxgi-two-pages.pdf");
    let bytes = completed_two_page_pdf(&target);
    let output_hash = Sha256::digest(bytes.as_slice());
    let output_hex: String = output_hash.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(result["output_bytes"].as_u64(), Some(bytes.len() as u64));
    let mut output = Automation::new().with_root(&allowed).unwrap();
    let printed = DocId(json(&mut output, "doc_open", json!({"path":"native-dxgi-two-pages.pdf"}))["doc"].as_u64().unwrap());
    let output_pixels = [output.render_pixels(printed.0, 1, 1.0).unwrap(), output.render_pixels(printed.0, 2, 1.0).unwrap()];
    let output_ink = distinct_ink(&output_pixels);
    assert_unchanged(&mut a, id, &before);
    assert_eq!(Sha256::digest(std::fs::read(&source_path).unwrap()), source_hash);

    let error = a.call("doc_print", &request).unwrap_err().to_string();
    assert!(error.contains("exists") || error.contains("replace"), "repeated target must fail without another job: {error}");
    assert_eq!(Sha256::digest(completed_two_page_pdf(&target).as_slice()), output_hash);
    assert_eq!(Sha256::digest(std::fs::read(&source_path).unwrap()), source_hash);
    assert_unchanged(&mut a, id, &before);
    let entries: Vec<_> = std::fs::read_dir(&allowed).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(
        entries,
        [std::ffi::OsString::from("native-dxgi-two-pages.pdf")],
        "only the one final output may remain; staging residue must be zero"
    );
    assert_authoring_history(&mut a, id, &before);
    assert_eq!(Sha256::digest(std::fs::read(&source_path).unwrap()), source_hash);

    let report = json!({"backend":result["backend"],"sheets":2,"output_bytes":bytes.len(),
        "output_sha256":output_hex,"source_sha256":source_hex,
        "source_texts":before.texts,"source_state":before.state,"output_region_ink":output_ink,
        "repair_free":true,"no_clobber":true,"source_preserved":true,"undo_redo_preserved":true,
        "staging_residue_in_allowed_root":0,"physical_print_verified":false});
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join("multipage-result.json"))
        .unwrap()
        .write_all(&serde_json::to_vec_pretty(&report).unwrap())
        .unwrap();
    // Keep the source, native two-page PDF and machine-readable report for coordinator review.
}
