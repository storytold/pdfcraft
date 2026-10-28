//! Packaging contracts for the installed browser app and its PDF file handler.
use std::path::Path;

/// Keeps the installed manifest, PDF handler, and copied icons within the hosting prefix.
#[test]
fn the_installed_web_app_declares_an_in_scope_pdf_file_handler() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = std::fs::read_to_string(root.join("manifest.webmanifest"))
        .expect("the installed app needs a shipped manifest, not only an ordinary browser page");
    let manifest: serde_json::Value = serde_json::from_str(&source).unwrap();
    assert_eq!(manifest["name"], "PdfCraft");
    assert_eq!(manifest["short_name"], "PdfCraft");
    assert_eq!(manifest["display"], "standalone");
    for member in ["scope", "start_url"] {
        assert_eq!(manifest[member], "./", "{member} must stay within the hosting prefix");
    }
    let handlers = manifest["file_handlers"].as_array().unwrap();
    assert_eq!(handlers.len(), 1);
    assert_eq!(handlers[0]["action"], "./");
    assert_eq!(handlers[0]["accept"], serde_json::json!({"application/pdf": [".pdf"]}));
    assert!(manifest.get("launch_handler").is_none(), "window routing stays under browser control");
    let icons = manifest["icons"].as_array().unwrap();
    assert!(!icons.is_empty());
    let html = std::fs::read_to_string(root.join("index.html")).unwrap();
    assert!(html.contains("rel=\"manifest\" href=\"manifest.webmanifest\""));
    assert!(html.contains("data-trunk rel=\"copy-file\" href=\"manifest.webmanifest\""));
    for icon in icons {
        let src = icon["src"].as_str().unwrap();
        assert!(!src.starts_with('/') && !src.contains(':') && !src.contains(".."));
        assert_eq!(icon["type"], "image/png");
        assert!(html.contains(&format!("data-target-path=\"{}\"", Path::new(src).parent().unwrap().display())), "Trunk must ship icon {src}");
    }
}

/// Leaves the manifest id unset so its prefix-local start URL determines app identity.
#[test]
fn a_static_manifest_uses_its_start_url_as_the_prefix_local_identity() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = std::fs::read_to_string(root.join("manifest.webmanifest")).unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&source).unwrap();
    // Manifest ids resolve against the origin, whereas start_url resolves against the
    // manifest URL. Omitting id preserves separate identities under arbitrary prefixes.
    assert!(manifest.get("id").is_none(), "a static origin-relative id collides across hosting prefixes");
    assert_eq!(manifest["start_url"], "./");
}
