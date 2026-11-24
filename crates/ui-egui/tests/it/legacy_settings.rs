//! The last release's `app.ron` becomes `config.toml` once.

use craft::settings::{Location, OpenOptions, SettingsStore};
use craft::theme::ModeChoice;
use pdfcraft_ui_egui::settings::{PdfCraftSettings, legacy_files_in};

/// A RON string literal holding `text`.
fn ron_string(text: &str) -> String {
    serde_json::to_string(text).unwrap()
}

#[test]
fn an_app_ron_from_the_last_release_imports_every_family_once() {
    let root = std::env::temp_dir().join(format!("pdfcraft-legacy-settings-{}", std::process::id()));
    let (first, second, config) = (root.join("first"), root.join("second"), root.join("config"));
    for dir in [&first, &second, &config] {
        std::fs::create_dir_all(dir).unwrap();
    }
    // The nearer folder holds only a PrintCraft entry; the stale one a PdfCraft entry that must not win.
    let printcraft = serde_json::json!({
        "author": "Printer",
        "recent_limit": 7,
        "reopen_last_session": false,
        "theme": "dark",
        "print": { "duplex": "long_edge" },
        "last_session": { "files": [{ "path": "/tmp/a.pdf" }], "active": 0 },
    })
    .to_string();
    let first_ron = format!("{{\"printcraft\": {}}}", ron_string(&printcraft));
    let second_ron = format!("{{\"pdfcraft\": {}}}", ron_string(r#"{"author":"Stale"}"#));
    std::fs::write(first.join("app.ron"), &first_ron).unwrap();
    std::fs::write(second.join("app.ron"), &second_ron).unwrap();
    let folders = [first.clone(), second.clone()];

    let options = || OpenOptions::<PdfCraftSettings>::new(Location::Dir(config.clone())).legacy(legacy_files_in(&folders));
    let (mut store, _report) = SettingsStore::open_with(options());
    {
        let s = store.get();
        assert_eq!(s.view.author.as_deref(), Some("Printer"), "the nearer folder's PrintCraft entry wins over a stale PdfCraft one");
        assert_eq!(s.recent.recent_limit, Some(7));
        assert_eq!(s.layout.reopen_last_session, Some(false));
        assert!(matches!(s.layout.theme, Some(ModeChoice::Dark)));
        assert_eq!(s.print.duplex.as_deref(), Some("long_edge"));
        let session = s.session.last_session.as_deref().unwrap_or_default();
        assert!(session.contains("documents") && !session.contains("\"files\""), "{session}");
    }
    store.flush().unwrap();
    assert!(config.join("config.toml").exists(), "the import is written as config.toml");
    assert_eq!(std::fs::read_to_string(first.join("app.ron")).unwrap(), first_ron, "the old file is untouched");

    // A second open reads config.toml: a changed old file is not imported again.
    let changed = format!("{{\"pdfcraft\": {}}}", ron_string(r#"{"author":"Changed"}"#));
    std::fs::write(first.join("app.ron"), changed).unwrap();
    let (again, _report) = SettingsStore::open_with(options());
    assert_eq!(again.get().view.author.as_deref(), Some("Printer"));
    let _ = std::fs::remove_dir_all(&root);
}
