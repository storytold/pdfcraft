//! CLI page commands must use the same dynamic XFA layout as the app and tools (#1645).

use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::Arc;

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

/// The synthetic dynamic form the other XFA tests use: a one-page placeholder PDF whose XFA
/// stream lays out into two pages.
const XDP: &str = r#"<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb">
 <pageSet><pageArea name="page"><occur max="-1"/>
  <contentArea x="20pt" y="20pt" w="160pt" h="260pt"/>
  <medium short="200pt" long="300pt"/>
 </pageArea></pageSet>
 <draw w="160pt" h="20pt"><value><text>First XFA page</text></value></draw>
 <field name="name" w="160pt" h="20pt"><ui><textEdit/></ui><value><text>Ada</text></value></field>
 <subform name="last" layout="tb"><breakBefore targetType="pageArea"/>
  <draw w="160pt" h="20pt"><value><text>Second XFA page</text></value></draw>
 </subform>
</subform></template></xdp:xdp>"#;

fn fixture(test: &str) -> PathBuf {
    fixture_with_xdp(test, XDP)
}

/// The same placeholder PDF, with a different XFA stream: a variant template (or no template
/// at all) is a different dynamic form for one test.
fn fixture_with_xdp(test: &str, xdp: &str) -> PathBuf {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R /NeedsRendering true /AcroForm << /XFA 5 0 R /Fields [] >> >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>".into(),
        "<< /Length 0 >>\nstream\n\nendstream".into(),
        format!("<< /Length {} >>\nstream\n{xdp}\nendstream", xdp.len()),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    let dir = std::env::temp_dir().join(format!("pdfcraft-cli-xfa-{test}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("form.pdf");
    std::fs::write(&path, bytes).unwrap();
    path
}

fn success(output: &Output) {
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn info_reports_the_laid_out_pages_and_fields() {
    let path = fixture("info");
    let original = std::fs::read(&path).unwrap();
    let output = Command::new(BIN).arg("info").arg(&path).output().unwrap();
    success(&output);
    let info: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(info["xfa_layout"]["pages"], 2, "the synthetic form lays out into two pages");
    assert_eq!(info["pages"], 2, "top-level page count must agree with the app");
    assert_eq!(info["fields"], 1);
    assert_eq!(info["pdf_pages"], 1);
    assert_eq!(info["acroform_fields"], 0);
    assert_eq!(info["first_page_pt"], serde_json::json!([200.0, 300.0]));
    assert_eq!(info["file_size"], original.len());
    assert_eq!(std::fs::read(path).unwrap(), original, "inspection must not write the layout to the input");
}

#[test]
fn text_reads_the_layout_including_a_page_absent_from_the_placeholder() {
    let path = fixture("text");
    let output = Command::new(BIN).arg("text").arg(&path).output().unwrap();
    success(&output);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("First XFA page") && text.contains("Second XFA page"), "{text:?}");
    let output = Command::new(BIN).arg("text").arg(&path).args(["--page", "2"]).output().unwrap();
    success(&output);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("Second XFA page") && !text.contains("First XFA page"), "{text:?}");
}

#[test]
fn render_matches_the_engine_layout_on_the_second_page() {
    use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind};
    let path = fixture("render");
    let original = std::fs::read(&path).unwrap();
    let mut session = pdfcraft_engine::Session::new();
    let id = session.open("form.pdf", None, Arc::new(original.clone()), None).unwrap();
    let doc = session.get(id).unwrap();
    assert_eq!(doc.info.pages.len(), 2);
    assert!(doc.text_lines(1).iter().any(|line| line.text.contains("Second XFA page")));
    let mut renderer = PageRenderer::new(doc.display.clone(), RenderConfig::default());
    let expected = renderer.render(RenderRequest { page: 1, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, preview: false });
    assert!(expected.error.is_none(), "{:?}", expected.error);
    assert!(expected.rgba.as_chunks::<4>().0.iter().filter(|[r, _, _, a]| *r < 128 && *a > 0).count() > 10, "the layout has visible ink");
    let out = path.with_extension("pam");
    let output = Command::new(BIN).arg("render").arg(&path).args(["--page", "2", "--dpi", "72", "--out"]).arg(&out).output().unwrap();
    success(&output);
    let actual = std::fs::read(out).unwrap();
    let header = format!("P7\nWIDTH {}\nHEIGHT {}\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n", expected.width, expected.height);
    assert!(actual.starts_with(header.as_bytes()));
    assert_eq!(&actual[header.len()..], &expected.rgba[..]);
    assert_eq!(std::fs::read(path).unwrap(), original);
}

#[test]
fn a_laid_out_form_keeps_the_original_copy_permissions() {
    let path = fixture("permissions");
    let mut doc = pdfcraft_cos::Document::open(Arc::new(std::fs::read(&path).unwrap())).unwrap();
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: "openme",
        owner_password: "boss",
        permissions: -17,
        encrypt_metadata: true,
        seed: [17; 32],
    })
    .unwrap();
    let original = pdfcraft_cos::write_full(&doc, &Default::default()).unwrap();
    std::fs::write(&path, &original).unwrap();
    let output = Command::new(BIN).arg("text").arg(&path).args(["--password", "openme"]).output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("don't allow copying"));
    let output = Command::new(BIN).arg("text").arg(&path).args(["--password", "boss", "--page", "2"]).output().unwrap();
    success(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("Second XFA page"));
    let output = Command::new(BIN).arg("info").arg(&path).args(["--password", "openme"]).output().unwrap();
    success(&output);
    let info: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(info["pages"], 2);
    assert_eq!(info["encrypted"], true);
    assert_eq!(info["file_size"], original.len());
    assert_eq!(std::fs::read(path).unwrap(), original);
}

/// `check` opens and renders what the app shows, so a dynamic XFA form is checked after it is
/// laid out: its two laid-out pages, not the one placeholder page the file's page tree holds
/// (#1645: page commands use the same layout).
#[test]
fn check_reports_and_renders_the_laid_out_pages_of_a_dynamic_xfa_form() {
    let path = fixture("check");
    let original = std::fs::read(&path).unwrap();
    let report = std::env::temp_dir().join(format!("pdfcraft-cli-xfa-check-report-{}.json", std::process::id()));
    let output = Command::new(BIN).arg("check").arg(&path).args(["--json"]).arg(&report).output().unwrap();
    success(&output);
    let results: Vec<serde_json::Value> = serde_json::from_slice(&std::fs::read(&report).unwrap()).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["file"], path.to_string_lossy().as_ref());
    assert_eq!(results[0]["pages"], 2, "check counts the laid-out pages, like `info`'s `pages`");
    assert_eq!(results[0]["status"], "ok");
    assert_eq!(results[0]["failed_pages"], 0);
    assert_eq!(std::fs::read(path).unwrap(), original, "checking must not write the layout to the input");
}

/// A dynamic form without a template still opens: the placeholder page stays and the reason is
/// a warning (the engine's
/// `an_xfa_form_without_a_template_opens_with_its_placeholder_and_a_warning`). That placeholder
/// renders, so `check` must not call the file ok — the layout failure is the finding, with its
/// reason as the detail.
#[test]
fn check_reports_a_failed_xfa_layout() {
    let path = fixture_with_xdp("check-failed-layout", "<xdp:xdp xmlns:xdp=\"http://ns.adobe.com/xdp/\"></xdp:xdp>");
    let original = std::fs::read(&path).unwrap();
    let report = std::env::temp_dir().join(format!("pdfcraft-cli-xfa-check-failed-report-{}.json", std::process::id()));
    let output = Command::new(BIN).arg("check").arg(&path).args(["--json"]).arg(&report).output().unwrap();
    success(&output);
    let results: Vec<serde_json::Value> = serde_json::from_slice(&std::fs::read(&report).unwrap()).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["status"], "xfa-layout-failed");
    assert_eq!(results[0]["pages"], 1, "only the placeholder page is there");
    assert_eq!(results[0]["failed_pages"], 1);
    let detail = results[0]["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("could not be laid out"), "{:?}", detail);
    assert!(results[0]["warnings"].as_u64().unwrap_or(0) >= 1, "the reason is among the warnings");
    assert_eq!(std::fs::read(path).unwrap(), original, "checking must not write the layout to the input");
}

/// A successful layout's warnings live in the layout report (`doc.xfa`), not in the file
/// inspection's `warnings`: this form's field value asks for a glyph the field's font can't
/// draw, laying it out draws `?` in its place and warns, and `check` must count that warning.
#[test]
fn check_counts_a_successful_layouts_own_warnings() {
    let value = XDP.replace("Ada", "\u{e000}");
    let path = fixture_with_xdp("check-layout-warnings", &value);
    let report = std::env::temp_dir().join(format!("pdfcraft-cli-xfa-check-warning-report-{}.json", std::process::id()));
    let output = Command::new(BIN).arg("check").arg(&path).args(["--json"]).arg(&report).output().unwrap();
    success(&output);
    let results: Vec<serde_json::Value> = serde_json::from_slice(&std::fs::read(&report).unwrap()).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["pages"], 2);
    assert_eq!(results[0]["status"], "ok");
    let warnings = results[0]["warnings"].as_u64().unwrap_or(0);
    assert!(warnings >= 1, "the layout's own warning is counted, head of detail: {:?}", results[0]["detail"]);
}
