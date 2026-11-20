//! Independent DXGI destination contracts. Default tests never enter a legacy native branch.
//! Real native requests are Windows-only, ignored and require a fresh explicit local root.
//! Native init callsite/HRESULT evidence is separate; argument rejection is not that RED.
use super::*;

const NFD: &str = "C\u{327}ag\u{306}rı I\u{307}stanbul S\u{327}is\u{327}li";
const PRINTER: &str = "Microsoft Print to PDF";

struct SourceSnapshot {
    bytes: Arc<Vec<u8>>,
    document: Value,
    text: String,
    pixels: (u32, u32, pdfcraft_render::Pixels),
}

fn source(root: &Path, pages: usize) -> (Automation, DocId) {
    let mut a = Automation::new().with_root(root).unwrap().with_clock(|| 1_700_000_000);
    let id = DocId(json(&mut a, "doc_create", json!({"from":"blank","pages":pages,"width":595.28,"height":841.89}))["doc"].as_u64().unwrap());
    json(&mut a, "page_add_text", json!({"doc":id.0,"page":1,"text":NFD,"at":[40,60],"width":500,"size":18}));
    (a, id)
}

fn extracted(a: &mut Automation, id: DocId) -> String {
    // Only outside CR/LF is ignored, never normalization or internal whitespace.
    json(a, "text_extract", json!({"doc":id.0,"pages":[1]}))["pages"][0]["text"].as_str().unwrap().trim_matches(['\r', '\n']).to_owned()
}

fn snapshot(a: &mut Automation, id: DocId) -> SourceSnapshot {
    let text = extracted(a, id);
    assert_eq!(text, NFD);
    let document = json(a, "doc_info", json!({"doc":id.0}))["document"].clone();
    let pixels = a.render_pixels(id.0, 1, 1.0).unwrap();
    assert!(pixels.2.chunks_exact(4).any(|p| p[3] > 0 && p[0] < 128 && p[1] < 128 && p[2] < 128), "source must contain actual NFD ink");
    SourceSnapshot { bytes: a.session.save_bytes(id).unwrap(), document, text, pixels }
}

fn assert_unchanged(a: &mut Automation, id: DocId, before: &SourceSnapshot) {
    assert_eq!(a.session.save_bytes(id).unwrap(), before.bytes, "printing cannot rewrite the source graph");
    assert_eq!(extracted(a, id), before.text, "printing cannot normalize source Unicode");
    assert!(a.render_pixels(id.0, 1, 1.0).unwrap() == before.pixels, "printing cannot change source ink");
    assert_eq!(json(a, "doc_info", json!({"doc":id.0}))["document"], before.document, "source dirty/path/undo/redo state must stay unchanged");
}

fn assert_single_authoring_undo(a: &mut Automation, id: DocId, before: &SourceSnapshot) {
    json(a, "edit_undo", json!({"doc":id.0}));
    assert_eq!(extracted(a, id), "", "one undo must remove the sole source text edit");
    assert!(a.call("edit_undo", &json!({"doc":id.0})).is_err(), "printing or rejection must not append hidden undo entries");
    json(a, "edit_redo", json!({"doc":id.0}));
    assert_eq!(extracted(a, id), before.text);
    assert!(a.render_pixels(id.0, 1, 1.0).unwrap() == before.pixels);
    assert_eq!(json(a, "doc_info", json!({"doc":id.0}))["document"], before.document);
}

fn dxgi_args(id: DocId, output: &str) -> Value {
    json!({"doc":id.0,"printer":PRINTER,"printer_output":output,"as_image":false,
        "pages":"1","paper":"A4","orientation":"portrait","layout":"actual",
        "copies":1,"duplex":"off","options":{}})
}

#[test]
fn ineligible_explicit_dxgi_requests_preserve_source_without_any_legacy_job() {
    let dir = tempfile::tempdir().unwrap();
    let (mut a, id) = source(dir.path(), 2);
    let before = snapshot(&mut a, id);
    let base = dxgi_args(id, "must-not-exist.pdf");
    for (key, value) in [
        ("path", json!("ordinary.pdf")),
        ("printer", json!("default")),
        ("printer", json!("impossible\0not-a-printer")),
        ("copies", json!(2)),
        ("duplex", json!("long-edge")),
        ("duplex", json!("printer")),
        ("options", json!({"media":"A4"})),
        ("printer_output", json!("")),
    ] {
        let mut args = base.clone();
        args[key] = value;
        // Every request has printer_output + as_image:false. The old implementation rejects
        // this before all native calls; the new contract must reject each ineligible request.
        let result = a.call("doc_print", &args);
        assert!(result.is_err(), "{args}: an ineligible destination must not start printing: {result:?}");
        assert_unchanged(&mut a, id, &before);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "rejection must publish neither output nor staging residue");
    }
    assert_single_authoring_undo(&mut a, id, &before);
}

#[test]
fn explicit_file_output_requires_an_explicit_image_mode() {
    let dir = tempfile::tempdir().unwrap();
    let (mut a, id) = source(dir.path(), 1);
    let before = snapshot(&mut a, id);
    let mut args = dxgi_args(id, "must-not-exist.pdf");
    args.as_object_mut().unwrap().remove("as_image");
    let error = a.call("doc_print", &args).unwrap_err().to_string();
    assert!(error.contains("as_image"), "omitting the backend choice must give an actionable error: {error}");
    assert_unchanged(&mut a, id, &before);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    assert_single_authoring_undo(&mut a, id, &before);
}

#[test]
fn malformed_driver_options_precede_headless_destination_checks_and_preserve_source() {
    let dir = tempfile::tempdir().unwrap();
    let (mut a, id) = source(dir.path(), 1);
    let before = snapshot(&mut a, id);
    for printer in [PRINTER, "default"] {
        for (options, message) in [(json!(["InputSlot"]), "printer_options"), (json!({"InputSlot":2}), "options.InputSlot")] {
            let error = a.call("doc_print", &json!({"doc":id.0,"printer":printer,"options":options})).unwrap_err();
            assert!(matches!(error, ToolError::InvalidArgs(_)), "{error}");
            assert!(error.to_string().contains(message), "malformed options must precede destination checks: {error}");
            assert_unchanged(&mut a, id, &before);
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        }
    }
    assert_single_authoring_undo(&mut a, id, &before);
}

#[cfg(windows)]
fn opt_in_root() -> PathBuf {
    let value = std::env::var_os("PDFCRAFT_REAL_DXGI_OUTPUT_ROOT")
        .expect("explicit native test requires PDFCRAFT_REAL_DXGI_OUTPUT_ROOT; absence must fail, never skip as PASS");
    let root = PathBuf::from(value);
    let local_drive = |path: &Path| {
        matches!(
            path.components().next(), Some(std::path::Component::Prefix(prefix))
                if matches!(prefix.kind(), std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_))
        )
    };
    assert!(root.is_absolute() && local_drive(&root), "opt-in root must be an absolute local drive directory");
    let root = root.canonicalize().expect("caller must prepare the explicit native evidence directory");
    assert!(root.is_dir() && local_drive(&root), "canonical evidence root must remain local");
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0, "choose a fresh empty evidence directory; existing outputs are never removed");
    root
}

#[cfg(windows)]
fn completed_pdf(path: &Path) -> Arc<Vec<u8>> {
    use std::io::Read;

    const LIMIT: u64 = 128 * 1024 * 1024;
    let file = std::fs::File::open(path).expect("DXGI success must publish a real completed output PDF");
    let metadata = file.metadata().unwrap();
    assert!(metadata.is_file() && metadata.len() <= LIMIT, "native output must be a bounded regular file");
    let mut bytes = Vec::new();
    file.take(LIMIT + 1).read_to_end(&mut bytes).unwrap();
    assert!(bytes.len() as u64 <= LIMIT);
    assert!(bytes.starts_with(b"%PDF-"), "native output needs an actual PDF header");
    let end = bytes.iter().rposition(|b| !b.is_ascii_whitespace()).map_or(0, |i| i + 1);
    assert!(bytes.get(..end).is_some_and(|b| b.ends_with(b"%%EOF")), "native output needs a complete EOF marker");
    let bytes = Arc::new(bytes);
    let doc = pdfcraft_cos::Document::open(bytes.clone()).unwrap();
    assert!(doc.repair_log().is_empty(), "native PDF must parse without repairs: {:?}", doc.repair_log());
    assert_eq!(pdfcraft_model::pages(&doc).len(), 1, "native output must contain exactly one imposed sheet");
    pdfcraft_render::check_pages(bytes.clone(), None, 1).unwrap();
    bytes
}

#[cfg(windows)]
#[test]
#[ignore = "sends one explicit local Microsoft Print to PDF DXGI job; PDFCRAFT_REAL_DXGI_OUTPUT_ROOT must be a fresh local directory"]
fn windows_dxgi_file_output_draws_nfd_without_gdi_fallback_and_preserves_source() {
    use sha2::{Digest, Sha256};

    let root = opt_in_root();
    // Keep confinement probes wholly inside the caller-selected evidence directory.
    let confined = root.join("allowed");
    std::fs::create_dir(&confined).unwrap();
    let (mut a, id) = source(&confined, 1);
    let before = snapshot(&mut a, id);
    let args = dxgi_args(id, "native-dxgi-nfd.pdf");
    let result = json(&mut a, "doc_print", args.clone());
    assert_eq!(result["backend"], "windows-dxgi", "a GDI response or missing backend cannot pass the DXGI regression: {result}");
    eprintln!("explicit DXGI native response: {result}");
    let target = confined.join("native-dxgi-nfd.pdf");
    let bytes = completed_pdf(&target);
    let hash = Sha256::digest(bytes.as_slice());
    let mut output = Automation::new().with_root(&confined).unwrap();
    let printed = DocId(json(&mut output, "doc_open", json!({"path":"native-dxgi-nfd.pdf"}))["doc"].as_u64().unwrap());
    let pixels = output.render_pixels(printed.0, 1, 1.0).unwrap();
    assert!(pixels.2.chunks_exact(4).any(|p| p[3] > 0 && p[0] < 128 && p[1] < 128 && p[2] < 128), "real native PDF must draw NFD text ink");
    // as_image:false requests DXGI; it does not promise searchable output from the driver.
    assert_unchanged(&mut a, id, &before);

    let error = a.call("doc_print", &args).unwrap_err().to_string();
    assert!(error.contains("exists") || error.contains("replace"), "repeating a final target must fail without clobber: {error}");
    assert_eq!(Sha256::digest(completed_pdf(&target).as_slice()), hash);
    assert_unchanged(&mut a, id, &before);

    let outside = root.join("outside-automation-root.pdf");
    assert!(!outside.exists());
    for path in ["../outside-automation-root.pdf".to_owned(), outside.to_string_lossy().into_owned()] {
        let error = a.call("doc_print", &dxgi_args(id, &path)).unwrap_err().to_string();
        assert!(error.contains("outside") || error.contains("root"), "the DXGI destination must enforce confinement before a job: {error}");
        assert!(!outside.exists());
        assert_eq!(Sha256::digest(completed_pdf(&target).as_slice()), hash);
        assert_unchanged(&mut a, id, &before);
    }
    assert_eq!(std::fs::read_dir(&confined).unwrap().count(), 1, "only the final native PDF may remain, no staging residue");
    assert_single_authoring_undo(&mut a, id, &before);
    // Leave allowed/native-dxgi-nfd.pdf in the persistent evidence root, never a TempDir.
}

#[cfg(windows)]
#[test]
#[ignore = "requires the integrated no-output guard and explicit fresh DXGI evidence root; old code can enter a real native job"]
fn windows_named_pdf_printer_without_destination_is_refused_with_actionable_error() {
    // Select this test only after the new guard is integrated. Running it on the old
    // implementation can enter the known long native initialization; default tests never do.
    let root = opt_in_root();
    let (mut a, id) = source(&root, 1);
    let before = snapshot(&mut a, id);
    let started = std::time::Instant::now();
    let error = a
        .call(
            "doc_print",
            &json!({"doc":id.0,"printer":PRINTER,"as_image":false,
        "pages":"1","paper":"A4","orientation":"portrait","copies":1,"duplex":"off","options":{}}),
        )
        .unwrap_err()
        .to_string();
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "the pure destination guard must not wait for native initialization");
    assert!(error.contains("printer_output"), "headless PDF printer calls need an explicit actionable destination guard: {error}");
    assert_unchanged(&mut a, id, &before);
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0, "the fast guard must publish no native or staging files");
    assert_single_authoring_undo(&mut a, id, &before);
}
