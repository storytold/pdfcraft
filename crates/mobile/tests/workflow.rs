use pdfcraft_mobile::create_mobile_session;
use pdfcraft_testkit::Pdf;
use serde_json::Value;

fn fixture() -> Vec<u8> {
    fixture_with_dimensions(200, 300)
}

fn fixture_with_dimensions(width: u32, height: u32) -> Vec<u8> {
    Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} {height}] /Contents 4 0 R >>"))
        .stream("", b"1 0 0 rg 10 20 60 40 re f")
        .build()
}

fn value(result: Result<String, String>) -> Value {
    serde_json::from_str(&result.unwrap()).unwrap()
}

#[test]
fn open_render_rotate_save_reopen_preserves_pixels_and_original() {
    let input = tempfile::tempdir().unwrap();
    let original = fixture();
    let source = input.path().join("input.pdf");
    std::fs::write(&source, &original).unwrap();
    let session = create_mobile_session().unwrap();
    let opened = value(session.open(source.to_str().unwrap()));
    assert_eq!(opened["page_count"], 1);
    assert_eq!(opened["pages"][0]["rotation"], 0);
    let before = value(session.render(1, 600));
    assert_eq!((before["width"].as_u64(), before["height"].as_u64()), (Some(400), Some(600)));
    let before_pixels = std::fs::read(before["path"].as_str().unwrap()).unwrap();

    let rotated = value(session.rotate(1));
    assert_eq!(rotated["pages"][0]["rotation"], 90);
    assert_eq!(rotated["dirty"], true);
    let after = value(session.render(1, 600));
    assert_eq!((after["width"].as_u64(), after["height"].as_u64()), (Some(600), Some(400)));
    let rotated_pixels = std::fs::read(after["path"].as_str().unwrap()).unwrap();
    assert_ne!(before_pixels, rotated_pixels);
    assert!(session.close(false).is_err(), "unsaved work must not be discarded");

    let saved = value(session.save_copy());
    let output = std::path::PathBuf::from(saved["path"].as_str().unwrap());
    assert_eq!(std::fs::read(&source).unwrap(), original);
    assert!(std::fs::read(&output).unwrap().starts_with(b"%PDF-"));
    value(session.close(false));
    let reopened = value(session.open(output.to_str().unwrap()));
    assert_eq!(reopened["pages"][0]["rotation"], 90);
    let rendered = value(session.render(1, 600));
    assert_eq!(std::fs::read(rendered["path"].as_str().unwrap()).unwrap(), rotated_pixels);
    let private_directory = output.parent().unwrap().to_path_buf();
    drop(session);
    assert!(!private_directory.exists(), "session destruction cleans previews and exports");
}

#[test]
fn invalid_input_and_pages_return_structured_errors_without_losing_document() {
    let input = tempfile::tempdir().unwrap();
    let source = input.path().join("input.pdf");
    std::fs::write(&source, b"not a PDF").unwrap();
    let session = create_mobile_session().unwrap();
    let error: Value = serde_json::from_str(&session.open(source.to_str().unwrap()).unwrap_err()).unwrap();
    assert_eq!(error["code"], "document");
    std::fs::write(&source, fixture()).unwrap();
    value(session.open(source.to_str().unwrap()));
    for (page, edge) in [(0, 600), (2, 600), (1, 0), (1, 2049)] {
        assert!(session.render(page, edge).is_err());
    }
    assert!(session.rotate(0).is_err());
    assert!(session.rotate(2).is_err());
    value(session.render(1, 600));
    value(session.close(false));
    assert!(session.render(1, 600).is_err());
}

#[test]
fn oversized_input_is_rejected_before_parsing() {
    let input = tempfile::tempdir().unwrap();
    let source = input.path().join("large.pdf");
    let file = std::fs::File::create(&source).unwrap();
    file.set_len(32 * 1024 * 1024 + 1).unwrap();
    let session = create_mobile_session().unwrap();
    let error: Value = serde_json::from_str(&session.open(source.to_str().unwrap()).unwrap_err()).unwrap();
    assert_eq!(error["code"], "limit");
}

#[test]
fn extreme_page_geometry_is_rejected_without_poisoning_session() {
    let input = tempfile::tempdir().unwrap();
    let source = input.path().join("geometry.pdf");
    std::fs::write(&source, fixture_with_dimensions(1, 10_000)).unwrap();
    let session = create_mobile_session().unwrap();
    let error: Value = serde_json::from_str(&session.open(source.to_str().unwrap()).unwrap_err()).unwrap();
    assert_eq!(error["code"], "limit");
    std::fs::write(&source, fixture()).unwrap();
    value(session.open(source.to_str().unwrap()));
    value(session.render(1, 600));
}
