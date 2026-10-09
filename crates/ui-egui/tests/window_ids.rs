//! Panels, areas and modals take absolute egui ids, which all windows share. Every one in the
//! interface goes through `windows::wid`, so a second window gets ids of its own.

use std::path::Path;

/// Files that may name plain ids: the theme is shared by all windows on purpose.
const EXEMPT_FILES: &[&str] = &["theme.rs", "windows.rs"];

fn sources(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn absolute_ids_are_window_scoped() {
    let mut files = Vec::new();
    sources(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
    assert!(files.len() > 20, "found the sources");
    let mut bad = Vec::new();
    for file in files {
        let name = file.file_name().unwrap().to_string_lossy().into_owned();
        if EXEMPT_FILES.contains(&name.as_str()) {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap();
        for (n, line) in text.lines().enumerate() {
            if ["Panel::top(\"", "Panel::left(\"", "Panel::right(\"", "Panel::bottom(\"", "Id::new(\""].iter().any(|p| line.contains(p))
                || line.contains("egui::Id::new(")
            {
                bad.push(format!("{name}:{}: {}", n + 1, line.trim()));
            }
        }
    }
    assert!(bad.is_empty(), "use windows::wid instead:\n{}", bad.join("\n"));
}
