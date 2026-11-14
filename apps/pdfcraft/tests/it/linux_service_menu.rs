//! The file managers' entries that put "Combine files in PdfCraft" on a file manager's right-click
//! menu (#585): a KDE service menu (Dolphin), a Nemo action, a libfm action (PCManFM) and a
//! Nautilus script. A `[Desktop Action]` in the application's .desktop file is only a launcher
//! quicklist entry with no selection, so each file manager needs its own file to offer the entry
//! on selected files.

use std::path::Path;

const APP_ID: &str = "ai.storyteller.pdfcraft";

fn read(relative: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::fs::read_to_string(root.join(relative)).unwrap_or_else(|e| panic!("{relative}: {e}"))
}

/// The `key=value` lines of one group, `None` when the file has no such group.
fn group<'a>(text: &'a str, name: &str) -> Option<Vec<(&'a str, &'a str)>> {
    let header = format!("[{name}]");
    let mut lines = text.lines().map(str::trim).skip_while(|line| *line != header);
    lines.next()?;
    Some(
        lines
            .take_while(|line| !line.starts_with('['))
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .filter_map(|line| line.split_once('='))
            .collect(),
    )
}

fn value<'a>(entries: &[(&'a str, &'a str)], key: &str) -> Option<&'a str> {
    entries.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

#[test]
fn combine_is_offered_for_pdfs_and_pictures() {
    let text = read(&format!("packaging/linux/kio-servicemenus/{APP_ID}-combine.desktop"));
    let entry = group(&text, "Desktop Entry").expect("a [Desktop Entry] group");
    assert_eq!(value(&entry, "Type"), Some("Service"));
    assert!(value(&entry, "X-KDE-Priority").is_some(), "a priority places the entry");

    let types: Vec<&str> = value(&entry, "MimeType").unwrap_or_default().split(';').filter(|t| !t.is_empty()).collect();
    for wanted in ["application/pdf", "image/png", "image/jpeg", "image/tiff", "image/gif", "image/bmp", "image/jp2"] {
        assert!(types.contains(&wanted), "{wanted} missing from {types:?}");
    }

    let actions: Vec<&str> = value(&entry, "Actions").unwrap_or_default().split(';').filter(|a| !a.is_empty()).collect();
    assert_eq!(actions, ["combine"]);
    let action = group(&text, "Desktop Action combine").expect("a [Desktop Action combine] group");
    assert_eq!(value(&action, "Name"), Some("Combine files in PdfCraft"));
    assert_eq!(value(&action, "Icon"), Some(APP_ID));
    // %F: every selected file as a local path, in the order the file manager lists them.
    assert_eq!(value(&action, "Exec"), Some("pdfcraft --combine %F"));
}

#[test]
fn the_service_menu_is_installed_with_the_linux_packages() {
    let script = read("packaging/linux/package.sh");
    assert!(script.contains("kio-servicemenus/$APP_ID-combine.desktop"), "package.sh must stage the service menu under usr/share/kio/servicemenus");
}

/// The MIME types of the KDE service menu, the one the others must match.
fn kde_types() -> Vec<String> {
    let text = read(&format!("packaging/linux/kio-servicemenus/{APP_ID}-combine.desktop"));
    let entry = group(&text, "Desktop Entry").expect("a [Desktop Entry] group");
    value(&entry, "MimeType").unwrap_or_default().split(';').filter(|t| !t.is_empty()).map(String::from).collect()
}

#[test]
fn nemo_offers_combine_for_the_same_types() {
    let text = read(&format!("packaging/linux/nemo-actions/{APP_ID}-combine.nemo_action"));
    let action = group(&text, "Nemo Action").expect("a [Nemo Action] group");
    assert_eq!(value(&action, "Name"), Some("Combine files in PdfCraft"));
    assert_eq!(value(&action, "Exec"), Some("pdfcraft --combine %F"));
    assert_eq!(value(&action, "Quote"), Some("double"), "paths with spaces reach the program whole");
    assert_eq!(value(&action, "Selection"), Some("NotNone"), "shown for one or many selected files, never for none");
    let types: Vec<String> = value(&action, "Mimetypes").unwrap_or_default().split(';').filter(|t| !t.is_empty()).map(String::from).collect();
    assert_eq!(types, kde_types());
}

#[test]
fn pcmanfm_offers_combine_for_the_same_types() {
    let text = read(&format!("packaging/linux/file-manager-actions/{APP_ID}-combine.desktop"));
    let entry = group(&text, "Desktop Entry").expect("a [Desktop Entry] group");
    assert_eq!(value(&entry, "Type"), Some("Action"));
    assert_eq!(value(&entry, "Name"), Some("Combine files in PdfCraft"));
    let profiles: Vec<&str> = value(&entry, "Profiles").unwrap_or_default().split(';').filter(|p| !p.is_empty()).collect();
    assert_eq!(profiles, ["combine"]);
    let profile = group(&text, "X-Action-Profile combine").expect("a profile group");
    assert_eq!(value(&profile, "Exec"), Some("pdfcraft --combine %F"));
    let types: Vec<String> = value(&profile, "MimeTypes").unwrap_or_default().split(';').filter(|t| !t.is_empty()).map(String::from).collect();
    assert_eq!(types, kde_types());
}

#[test]
fn the_nautilus_script_passes_every_selected_file_to_combine() {
    let script = read("packaging/linux/nautilus-scripts/Combine files in PdfCraft");
    assert!(script.starts_with("#!/bin/sh"), "a script Nautilus can run");
    assert!(script.lines().any(|line| line == "exec pdfcraft --combine \"$@\""), "{script}");
}

#[test]
fn every_file_manager_entry_is_installed_with_the_linux_packages() {
    let script = read("packaging/linux/package.sh");
    for staged in [
        "kio-servicemenus/$APP_ID-combine.desktop\" \"$STAGE/usr/share/kio/servicemenus/",
        "nemo-actions/$APP_ID-combine.nemo_action\" \"$STAGE/usr/share/nemo/actions/",
        "file-manager-actions/$APP_ID-combine.desktop\" \"$STAGE/usr/share/file-manager/actions/",
        "nautilus-scripts/Combine files in PdfCraft\" \"$STAGE/usr/share/pdfcraft/nautilus-scripts/",
    ] {
        assert!(script.contains(staged), "package.sh must stage {staged}");
    }
}
