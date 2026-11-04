//! Build-time configuration tests; they neither query GitHub nor install an update.

#[path = "../build/update_source.rs"]
mod update_source;

#[test]
fn compiled_source_is_shared_by_native_updates_and_the_release_link() {
    use pdfcraft_ui_egui::updates::{RELEASES_PAGE, REPOSITORY};

    assert_eq!(REPOSITORY, env!("PDFCRAFT_UPDATE_REPO"));
    assert_eq!(RELEASES_PAGE, format!("https://github.com/{REPOSITORY}/releases"));
    #[cfg(not(target_arch = "wasm32"))]
    {
        assert_eq!(pdfcraft_ui_egui::UPDATE_APP.repo, REPOSITORY);
        assert_eq!(pdfcraft_ui_egui::UPDATE_APP.releases_url(), RELEASES_PAGE);
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn release_assets_stay_inside_the_compiled_repository() {
    let repo = pdfcraft_ui_egui::UPDATE_APP.repo;
    let foreign = format!("{repo}-other");
    let allowed = format!("https://github.com/{repo}/releases/download/v99.0.0/pdfcraft-99.0.0-linux-x86_64.tar.gz");
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
