//! Build-time configuration tests; they neither query GitHub nor install an update.

#[path = "../build/update_source.rs"]
mod update_source;

/// Native checks and web release links must share the independently expected build-time source.
#[test]
fn compiled_source_is_shared_by_native_updates_and_the_release_link() {
    use pdfcraft_ui_egui::updates::{RELEASES_PAGE, REPOSITORY};

    let expected = std::env::var("PDFCRAFT_EXPECTED_UPDATE_REPO").unwrap_or_else(|_| "storytold/pdfcraft".to_owned());
    assert_eq!(REPOSITORY, expected, "compiled source must match the independent test expectation");
    assert_eq!(RELEASES_PAGE, format!("https://github.com/{REPOSITORY}/releases"));
    #[cfg(not(target_arch = "wasm32"))]
    {
        assert_eq!(pdfcraft_ui_egui::UPDATE_APP.repo, REPOSITORY);
        assert_eq!(pdfcraft_ui_egui::UPDATE_APP.releases_url(), RELEASES_PAGE);
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// Accept only the configured repository while refusing foreign and traversing download links.
#[test]
fn release_assets_stay_inside_the_compiled_repository() {
    let repo = pdfcraft_ui_egui::UPDATE_APP.repo;
    let foreign = format!("{repo}-other");
    let canonical = repo.to_ascii_lowercase();
    let allowed = format!("https://github.com/{canonical}/releases/download/v99.0.0/pdfcraft-99.0.0-linux-x86_64.tar.gz");
    let json = serde_json::json!({
        "tag_name": "v99.0.0",
        "html_url": format!("https://github.com/{foreign}/releases/tag/v99.0.0"),
        "assets": [
            { "name": "pdfcraft-99.0.0-linux-x86_64.tar.gz", "browser_download_url": allowed, "size": 1 },
            { "name": "foreign.tar.gz", "browser_download_url": format!("https://github.com/{foreign}/releases/download/v99.0.0/foreign.tar.gz"), "size": 1 },
            { "name": "escape.tar.gz", "browser_download_url": format!("https://github.com/{repo}/releases/download/../../../another-owner/another-repo/releases/download/v99.0.0/escape.tar.gz"), "size": 1 }
        ]
    });
    let release = craft_update::parse_github_release(&json.to_string(), repo).unwrap();
    assert_eq!(release.html_url, pdfcraft_ui_egui::updates::RELEASES_PAGE);
    assert_eq!(release.assets.len(), 1);
    assert_eq!(release.assets.first().unwrap().url, allowed);
}

#[cfg(not(target_arch = "wasm32"))]
/// Exercise identity casing even when this test binary was built with the lowercase default.
#[test]
fn mixed_case_repository_accepts_only_its_own_assets() {
    let allowed = "https://github.com/example/pdfcraft/releases/download/v99.0.0/pdfcraft.tar.gz";
    let json = serde_json::json!({
        "tag_name": "v99.0.0",
        "html_url": "https://github.com/foreign/pdfcraft/releases/tag/v99.0.0",
        "assets": [
            {"name": "pdfcraft.tar.gz", "browser_download_url": allowed, "size": 1},
            {"name": "foreign.tar.gz", "browser_download_url": "https://github.com/example/pdfcraft-other/releases/download/v99.0.0/foreign.tar.gz", "size": 1}
        ]
    });
    let release = craft_update::parse_github_release(&json.to_string(), "Example/PdfCraft").unwrap();
    assert_eq!(release.assets.len(), 1);
    assert_eq!(release.assets[0].url, allowed);
    assert_eq!(release.html_url, "https://github.com/Example/PdfCraft/releases");
}
