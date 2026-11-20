//! Default tests export print-ready PDFs without jobs; one ignored Windows test opts in.
//! Ordinary fixtures use tempdirs; the opt-in native output stays in the chosen evidence root.
use super::*;
use sha2::{Digest, Sha256};

const NFC: [&str; 2] = ["Çağrı İstanbul Şişli Öğrenci GÜNEŞ ığdır", "İŞ Sözleşmesi"];
const NFD: [&str; 2] = [
    "C\u{327}ag\u{306}rı I\u{307}stanbul S\u{327}is\u{327}li O\u{308}g\u{306}renci GU\u{308}NES\u{327} ıg\u{306}dır",
    "I\u{307}S\u{327} So\u{308}zles\u{327}mesi",
];
const IMPOSSIBLE_PRINTER: &str = "p3-print-test\0cannot-name-a-Windows-printer";

fn hash(path: &Path) -> String {
    Sha256::digest(std::fs::read(path).unwrap()).iter().map(|b| format!("{b:02x}")).collect()
}

fn file_snapshot(root: &Path) -> Vec<(PathBuf, String)> {
    std::fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_file())
        .map(|p| {
            let digest = hash(&p);
            (p, digest)
        })
        .collect()
}

fn assert_files_unchanged(files: &[(PathBuf, String)]) {
    for (path, before) in files {
        assert_eq!(&hash(path), before, "printing must leave existing fixture/output bytes alone: {}", path.display());
    }
}

fn open(root: &Path, path: &str, password: Option<&str>) -> (Automation, DocId) {
    let mut a = Automation::new().with_root(root).unwrap().with_clock(|| 1_700_000_000);
    let mut args = json!({"path":path});
    if let Some(password) = password {
        args["password"] = json!(password);
    }
    let id = DocId(json(&mut a, "doc_open", args)["doc"].as_u64().unwrap());
    (a, id)
}

fn text(a: &mut Automation, id: DocId, page: usize) -> String {
    // Only outside CR/LF may be discarded. No NFC conversion or internal whitespace folding.
    json(a, "text_extract", json!({"doc":id.0,"pages":[page]}))["pages"][0]["text"].as_str().unwrap().trim_matches(['\r', '\n']).to_owned()
}

fn authored(root: &Path, texts: [&str; 2], stem: &str) -> (Automation, DocId) {
    let mut a = Automation::new().with_root(root).unwrap().with_clock(|| 1_700_000_000);
    let id = json(&mut a, "doc_create", json!({"from":"blank","pages":2,"width":612,"height":792}))["doc"].as_u64().unwrap();
    for (page, text) in texts.iter().enumerate() {
        let result = json(&mut a, "page_add_text", json!({"doc":id,"page":page+1,"text":text,"at":[40,60],"width":532,"size":16}));
        assert!(result["warnings"].as_array().is_none_or(Vec::is_empty), "{result}");
    }
    let path = format!("{stem} – C\u{327}ag\u{306}rı.pdf");
    json(&mut a, "doc_save", json!({"doc":id,"path":path}));
    open(root, &path, None)
}

fn editable_ocr(root: &Path, texts: [&str; 2], stem: &str) -> (Automation, DocId) {
    use pdfcraft_engine::ocr::{OcrPage, PlacedWord};

    // Original textless scan; the editable output must discard this dark border.
    let pixels: Vec<u8> = (0..792).flat_map(|y| (0..612).map(move |x| if x < 3 || y < 3 { 30 } else { 255 })).collect();
    let image = craft_image_codecs::Image::from_u8(612, 792, craft_image_codecs::ChannelLayout::Gray, pixels).unwrap();
    let png = craft_image_codecs::encode(&image, craft_image_codecs::Format::Png, &craft_image_codecs::EncodeOptions::default()).unwrap();
    let image = format!("{stem}-scan.png");
    std::fs::write(root.join(&image), png).unwrap();
    let mut a = Automation::new().with_root(root).unwrap().with_clock(|| 1_700_000_000);
    let id = DocId(json(&mut a, "doc_create", json!({"from":"images","paths":[image,image],"dpi":72}))["doc"].as_u64().unwrap());
    json(&mut a, "doc_save", json!({"doc":id.0,"path":format!("{stem}-scan.pdf")}));
    let before = a.session.save_bytes(id).unwrap();
    let files = file_snapshot(root);
    let guard = a.session.ocr_guard(id).unwrap();
    a.next_ocr_review += 1;
    let review = a.next_ocr_review;
    let pages = texts
        .iter()
        .enumerate()
        .map(|(page, _)| OcrPage {
            page,
            words: vec![PlacedWord {
                text: "Recognized".into(),
                origin: [40.0, 700.0],
                across: [520.0, 0.0],
                up: [0.0, 24.0],
                confidence: Some(45.0),
                source: "synthetic".into(),
            }],
            already_on_page: 0,
            skipped: None,
            notes: Vec::new(),
        })
        .collect();
    a.ocr_reviews.insert(id, OcrReview { handle: review, guard, pages, settings: Default::default() });
    let path = format!("{stem} – editable C\u{327}ag\u{306}rı.pdf");
    let corrections: Vec<_> = texts.iter().enumerate().map(|(p, t)| json!({"page":p+1,"word":1,"text":t})).collect();
    json(&mut a, "ocr_review_apply", json!({"doc":id.0,"review":review,"corrections":corrections,"output_mode":"editable","out":path}));
    assert_eq!(a.session.save_bytes(id).unwrap(), before);
    assert_files_unchanged(&files);
    let (b, out) = open(root, &path, None);
    assert!(b.session.get(out).unwrap().page_images(0).is_empty(), "print input is the real editable PDF, not its scan");
    (b, out)
}

fn print_ready(a: &mut Automation, id: DocId, root: &Path, path: &str, extra: Value) -> (Automation, DocId, Value) {
    let mut args = json!({"doc":id.0,"path":path,"layout":"actual","paper":"Letter","orientation":"portrait","comments_forms":"document"});
    for (key, value) in extra.as_object().unwrap() {
        args[key] = value.clone();
    }
    let result = json(a, "doc_print", args);
    assert!(result.get("job").is_none(), "a path export is not a submitted job: {result}");
    let bytes = std::fs::read(root.join(path)).unwrap();
    assert!(bytes.starts_with(b"%PDF-"));
    assert_eq!(result["bytes"].as_u64(), Some(bytes.len() as u64));
    let pdf = pdfcraft_cos::Document::open(Arc::new(bytes)).unwrap();
    assert_eq!(pdfcraft_model::pages(&pdf).len() as u64, result["sheets"].as_u64().unwrap());
    let (out, opened) = open(root, path, None);
    (out, opened, result)
}

fn assert_vector_print(root: &Path, a: &mut Automation, id: DocId, texts: [&str; 2], stem: &str) {
    let before = a.session.save_bytes(id).unwrap();
    let state = json(a, "doc_info", json!({"doc":id.0}))["document"].clone();
    let pixels: Vec<_> = (1..=2).map(|p| a.render_pixels(id.0, p, 1.0).unwrap()).collect();
    for (page, expected) in texts.iter().enumerate() {
        assert_eq!(text(a, id, page + 1), *expected);
        assert!(pixels[page].2.chunks_exact(4).any(|p| p[0] < 128 && p[1] < 128 && p[2] < 128));
    }
    for reverse in [false, true] {
        let files = file_snapshot(root);
        let path = format!("{stem}-print-{reverse} – I\u{307}S\u{327}.pdf");
        let (mut out, printed, result) = print_ready(a, id, root, &path, json!({"reverse":reverse}));
        assert_eq!(result["pages"], 2);
        assert_eq!(result["sheets"], 2);
        for sheet in 0..2 {
            let source = if reverse { 1 - sheet } else { sheet };
            assert_eq!(text(&mut out, printed, sheet + 1), texts[source], "{stem}, sheet {sheet}, reverse={reverse}");
            let page = &out.session.get(printed).unwrap().info.pages[sheet];
            assert_eq!((page.width, page.height, page.rotation), (612.0, 792.0, 0));
            assert!(
                out.render_pixels(printed.0, sheet + 1, 1.0).unwrap() == pixels[source],
                "actual Letter print must preserve source pixels: {stem}, sheet {sheet}"
            );
            let selected = out.page_texts(printed, &[sheet]).unwrap().remove(0);
            assert_eq!(selected.text_of(0..selected.glyphs.len()), texts[source], "selection keeps original Unicode through page Form XObjects");
        }
        let found = json(&mut out, "text_find", json!({"doc":printed.0,"query":"Şişli","language":"tr","case_sensitive":true}));
        assert_eq!(found["count"], 1, "{found}");
        assert_eq!(found["matches"][0]["text"], if texts == NFD { "S\u{327}is\u{327}li" } else { "Şişli" });
        assert_files_unchanged(&files);
        assert_eq!(a.session.save_bytes(id).unwrap(), before);
        assert_eq!(json(a, "doc_info", json!({"doc":id.0}))["document"], state);
    }
}

#[test]
fn saved_turkish_nfc_nfd_print_ready_keeps_exact_text_order_selection_and_pixels() {
    for (stem, texts) in [("authored-nfc", NFC), ("authored-nfd", NFD)] {
        let dir = tempfile::tempdir().unwrap();
        let (mut a, id) = authored(dir.path(), texts, stem);
        assert_vector_print(dir.path(), &mut a, id, texts, stem);
    }
}

#[test]
fn saved_editable_ocr_nfc_nfd_print_ready_keeps_exact_text_order_selection_and_pixels() {
    for (stem, texts) in [("ocr-nfc", NFC), ("ocr-nfd", NFD)] {
        let dir = tempfile::tempdir().unwrap();
        let (mut a, id) = editable_ocr(dir.path(), texts, stem);
        assert_vector_print(dir.path(), &mut a, id, texts, stem);
    }
}

#[test]
fn turkish_nfd_print_as_image_keeps_pixels_instead_of_searchable_text() {
    for ocr in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let (mut a, id) = if ocr { editable_ocr(dir.path(), NFD, "image-ocr") } else { authored(dir.path(), NFD, "image-text") };
        let before = a.session.save_bytes(id).unwrap();
        let files = file_snapshot(dir.path());
        let (mut out, printed, result) = print_ready(&mut a, id, dir.path(), "Baskı – resim.pdf", json!({"as_image":true,"dpi":72}));
        assert_eq!(result["sheets"], 2);
        for page in 1..=2 {
            assert_eq!(text(&mut out, printed, page), "", "image printing deliberately carries no text layer");
            assert!(
                out.render_pixels(printed.0, page, 1.0).unwrap() == a.render_pixels(id.0, page, 1.0).unwrap(),
                "72 dpi actual-size print must keep NFD ink, ocr={ocr}, page={page}"
            );
        }
        let pdf = pdfcraft_cos::Document::open(out.session.save_bytes(printed).unwrap()).unwrap();
        let images: Vec<_> = pdf
            .scan_objects()
            .filter_map(|(_, o)| match &*o {
                pdfcraft_cos::Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Image") => Some(s.dict.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(images.len(), 2);
        assert!(images.iter().all(|d| d.int(b"Width") == Some(612) && d.int(b"Height") == Some(792)));
        assert_eq!(a.session.save_bytes(id).unwrap(), before);
        assert_files_unchanged(&files);
    }
}

#[test]
fn rejected_print_arguments_keep_source_and_existing_output_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let (mut a, id) = authored(dir.path(), NFD, "invalid");
    std::fs::write(dir.path().join("existing.pdf"), b"existing-output-sentinel").unwrap();
    let files = file_snapshot(dir.path());
    let before = a.session.save_bytes(id).unwrap();
    let state = json(&mut a, "doc_info", json!({"doc":id.0}))["document"].clone();
    for mut args in [
        json!({}),
        // The impossible NUL name is never submitted: both target arguments must be refused.
        json!({"path":"existing.pdf","printer":IMPOSSIBLE_PRINTER}),
        json!({"path":"existing.pdf","pages":"99"}),
        json!({"path":"existing.pdf","subset":"unknown"}),
        json!({"path":"existing.pdf","layout":"unknown"}),
        json!({"path":"existing.pdf","layout":"custom","scale":0}),
        json!({"path":"existing.pdf","orientation":"unknown"}),
        json!({"path":"existing.pdf","comments_forms":"unknown"}),
        json!({"path":"existing.pdf","paper":"ARCH Z"}),
        json!({"path":"existing.pdf","as_image":true,"dpi":20}),
        json!({"path":"existing.pdf","layout":"multiple","order":"cut-stack","duplex":"long-edge"}),
    ] {
        args["doc"] = json!(id.0);
        let result = a.call("doc_print", &args);
        assert!(matches!(&result, Err(ToolError::InvalidArgs(_))), "{args}: {result:?}");
        assert_files_unchanged(&files);
        assert_eq!(a.session.save_bytes(id).unwrap(), before);
        assert_eq!(json(&mut a, "doc_info", json!({"doc":id.0}))["document"], state);
    }
    assert_vector_print(dir.path(), &mut a, id, NFD, "valid-retry");
}

#[test]
fn refused_print_paths_preserve_confined_files_source_and_failed_write_directory() {
    let outer = tempfile::tempdir().unwrap();
    let root = outer.path().join("root");
    std::fs::create_dir(&root).unwrap();
    let (mut a, id) = authored(&root, NFD, "confined");
    let outside = outer.path().join("outside.pdf");
    std::fs::write(&outside, b"outside-root-sentinel").unwrap();
    std::fs::create_dir(root.join("busy.pdf")).unwrap();
    std::fs::write(root.join("busy.pdf/keep.txt"), b"directory-sentinel").unwrap();
    let before = a.session.save_bytes(id).unwrap();
    let files = file_snapshot(&root);
    for path in ["../outside.pdf".to_owned(), "../missing.pdf".to_owned(), outside.to_string_lossy().into_owned(), "busy.pdf".to_owned()] {
        assert!(a.call("doc_print", &json!({"doc":id.0,"path":path,"layout":"actual"})).is_err(), "{path}");
        assert_eq!(a.session.save_bytes(id).unwrap(), before);
        assert_files_unchanged(&files);
        assert_eq!(std::fs::read(&outside).unwrap(), b"outside-root-sentinel");
        assert_eq!(std::fs::read(root.join("busy.pdf/keep.txt")).unwrap(), b"directory-sentinel");
        assert!(!outer.path().join("missing.pdf").exists());
    }
    assert_vector_print(&root, &mut a, id, NFD, "confined-retry");
}

#[test]
fn printing_denied_in_saved_turkish_pdf_cannot_publish_vector_or_image_output() {
    let dir = tempfile::tempdir().unwrap();
    let (mut owner, id) = authored(dir.path(), NFD, "permission-none");
    json(
        &mut owner,
        "doc_protect",
        json!({"doc":id.0,"open_password":"synthetic-reader","permissions_password":"synthetic-owner","printing":"none","copy":true}),
    );
    json(&mut owner, "doc_save", json!({"doc":id.0,"path":"Korumalı – NFD.pdf"}));
    let (mut a, restricted) = open(dir.path(), "Korumalı – NFD.pdf", Some("synthetic-reader"));
    assert_eq!(json(&mut a, "doc_info", json!({"doc":restricted.0}))["security"]["printing"], "none");
    let before = a.session.save_bytes(restricted).unwrap();
    std::fs::write(dir.path().join("existing.pdf"), b"existing-output-sentinel").unwrap();
    let files = file_snapshot(dir.path());
    for (path, as_image) in [("forbidden.pdf", false), ("existing.pdf", true)] {
        let result = a.call("doc_print", &json!({"doc":restricted.0,"path":path,"as_image":as_image,"dpi":72}));
        assert!(matches!(&result,Err(ToolError::Failed(m)) if m.contains("printing")), "{result:?}");
        assert_eq!(a.session.save_bytes(restricted).unwrap(), before);
        assert_eq!(text(&mut a, restricted, 1), NFD[0]);
        assert_files_unchanged(&files);
        assert!(!dir.path().join("forbidden.pdf").exists());
    }
}

#[test]
fn low_quality_print_permission_forces_bounded_images_even_when_copying_is_denied() {
    let dir = tempfile::tempdir().unwrap();
    let (mut owner, id) = authored(dir.path(), NFD, "permission-low");
    json(
        &mut owner,
        "doc_protect",
        json!({"doc":id.0,"open_password":"synthetic-reader","permissions_password":"synthetic-owner","printing":"low","copy":false}),
    );
    json(&mut owner, "doc_save", json!({"doc":id.0,"path":"Düşük kalite – NFD.pdf"}));
    let (mut a, restricted) = open(dir.path(), "Düşük kalite – NFD.pdf", Some("synthetic-reader"));
    let info = json(&mut a, "doc_info", json!({"doc":restricted.0}));
    assert_eq!(info["security"]["printing"], "low");
    assert_eq!(info["security"]["copy"], false);
    assert!(a.call("text_extract", &json!({"doc":restricted.0})).is_err());
    let before = a.session.save_bytes(restricted).unwrap();
    for as_image in [false, true] {
        let files = file_snapshot(dir.path());
        let (mut out, printed, _) =
            print_ready(&mut a, restricted, dir.path(), &format!("limited-{as_image}.pdf"), json!({"as_image":as_image,"dpi":600}));
        assert_eq!(text(&mut out, printed, 1), "");
        let pdf = pdfcraft_cos::Document::open(out.session.save_bytes(printed).unwrap()).unwrap();
        let images: Vec<_> = pdf
            .scan_objects()
            .filter_map(|(_, o)| match &*o {
                pdfcraft_cos::Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Image") => Some(s.dict.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(images.len(), 2);
        // Engine's existing low-quality print cap is 150 dpi, regardless of the 600 request.
        assert!(images.iter().all(|d| d.int(b"Width") == Some(1275) && d.int(b"Height") == Some(1650)), "{images:?}");
        assert!(out.render_pixels(printed.0, 1, 1.0).unwrap().2.chunks_exact(4).any(|p| p[0] < 128 && p[1] < 128 && p[2] < 128));
        assert_eq!(a.session.save_bytes(restricted).unwrap(), before);
        assert_files_unchanged(&files);
    }
}

#[test]
fn printer_output_must_not_be_silently_ignored_when_a_print_ready_path_is_present() {
    let dir = tempfile::tempdir().unwrap();
    let (mut a, id) = authored(dir.path(), NFD, "target-conflict");
    let files = file_snapshot(dir.path());
    let before = a.session.save_bytes(id).unwrap();
    // No printer, so this can never submit a job. Before the new schema exists, the
    // unknown argument is already rejected; afterward the handler must refuse the conflict.
    // This guards future handling, not a reproduced old-version silent-ignore bug.
    let result = a.call("doc_print", &json!({"doc":id.0,"path":"x.pdf","printer_output":"y.pdf"}));
    assert!(matches!(&result, Err(ToolError::InvalidArgs(_))), "path + printer_output must be rejected, not silently save x.pdf: {result:?}");
    assert!(!dir.path().join("x.pdf").exists());
    assert!(!dir.path().join("y.pdf").exists());
    assert_eq!(a.session.save_bytes(id).unwrap(), before);
    assert_files_unchanged(&files);
}

#[test]
fn ineligible_printer_output_arguments_are_refused_without_any_native_job() {
    let dir = tempfile::tempdir().unwrap();
    let (mut a, id) = authored(dir.path(), NFD, "ineligible-output");
    let before = a.session.save_bytes(id).unwrap();
    let files = file_snapshot(dir.path());
    for mut args in [
        json!({"printer_output":"y.pdf","as_image":false}),
        json!({"printer_output":"y.pdf","as_image":true}),
        // Unknown schema rejects these today; both old handler targets are also present,
        // so neither named/default request can spool if that schema check is bypassed.
        // These compound refusals do not independently prove each future native preflight.
        json!({"path":"x.pdf","printer_output":"y.pdf","as_image":true,"printer":"default"}),
        json!({"path":"x.pdf","printer_output":"y.pdf","as_image":true,"printer":IMPOSSIBLE_PRINTER}),
    ] {
        args["doc"] = json!(id.0);
        let result = a.call("doc_print", &args);
        assert!(matches!(&result, Err(ToolError::InvalidArgs(_))), "{args}: {result:?}");
        assert!(!dir.path().join("x.pdf").exists());
        assert!(!dir.path().join("y.pdf").exists());
        assert_eq!(a.session.save_bytes(id).unwrap(), before);
        assert_files_unchanged(&files);
    }
}

#[cfg(windows)]
#[test]
#[ignore = "sends one explicit local Microsoft Print to PDF job; opt-in fixture directory required"]
fn windows_virtual_printer_file_output_draws_turkish_nfd() {
    let chosen = std::env::var_os("PDFCRAFT_REAL_PRINT_OUTPUT_ROOT")
        .expect("opt-in native test requires PDFCRAFT_REAL_PRINT_OUTPUT_ROOT; no default directory and no PASS-by-skip");
    let root = PathBuf::from(chosen);
    let local_drive = |path: &Path| {
        matches!(
            path.components().next(),
            Some(std::path::Component::Prefix(prefix))
                if matches!(prefix.kind(), std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_))
        )
    };
    assert!(root.is_absolute() && local_drive(&root), "native output root must be an explicit absolute local drive path");
    let root = root.canonicalize().expect("opt-in native output root must already exist");
    assert!(root.is_dir() && local_drive(&root), "native output root must resolve to an existing local directory");
    let target = root.join("native-nfd.pdf");
    assert!(!target.exists(), "choose a fresh opt-in directory: native-nfd.pdf must not already exist");

    // One in-memory synthetic page, no source PDF written and no temporary output root.
    // The final native PDF intentionally remains in the caller-selected evidence directory.
    let original = "C\u{327}ag\u{306}rı I\u{307}stanbul S\u{327}is\u{327}li";
    let mut a = Automation::new().with_root(&root).unwrap().with_clock(|| 1_700_000_000);
    let id = DocId(json(&mut a, "doc_create", json!({"from":"blank","pages":1,"width":595.28,"height":841.89}))["doc"].as_u64().unwrap());
    json(&mut a, "page_add_text", json!({"doc":id.0,"page":1,"text":original,"at":[40,60],"width":500,"size":18}));
    assert_eq!(text(&mut a, id, 1), original);
    let before = a.session.save_bytes(id).unwrap();
    let state = json(&mut a, "doc_info", json!({"doc":id.0}))["document"].clone();

    // This is the only test in this module allowed to request a real job, and it is ignored
    // unless explicitly selected. Exact local queue, image route, one sheet/copy, duplex off.
    json(
        &mut a,
        "doc_print",
        json!({"doc":id.0,"printer_output":"native-nfd.pdf","as_image":true,
        "printer":"Microsoft Print to PDF","paper":"A4","duplex":"off","dpi":300,"copies":1}),
    );
    let bytes = std::fs::read(&target).expect("native job must publish its final PDF before doc_print succeeds");
    assert!(bytes.starts_with(b"%PDF-"));
    let pdf = pdfcraft_cos::Document::open(Arc::new(bytes)).unwrap();
    assert_eq!(pdfcraft_model::pages(&pdf).len(), 1, "native output must contain exactly one imposed sheet");
    let (mut output, printed) = open(&root, "native-nfd.pdf", None);
    let raster = output.render_pixels(printed.0, 1, 1.0).unwrap();
    assert!(raster.2.chunks_exact(4).any(|p| p[0] < 128 && p[1] < 128 && p[2] < 128), "native raster PDF must contain visible NFD text ink");
    // Raster printing has no extraction/search promise. Unicode belongs to the source here.
    assert_eq!(text(&mut a, id, 1), original);
    assert_eq!(a.session.save_bytes(id).unwrap(), before);
    assert_eq!(json(&mut a, "doc_info", json!({"doc":id.0}))["document"], state, "printing must preserve source bytes and undo history");
}
