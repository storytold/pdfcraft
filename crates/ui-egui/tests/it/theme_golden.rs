//! Golden dumps of the installed egui styles and theme tokens for every theme preference and
//! system answer, under `tests/fixtures/theme/`. Regenerate with `UPDATE_GOLDEN=1` and review
//! the diff: a changed fixture is a visible change to the app's colours or style.

use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;
use pdfcraft_ui_egui::theme::{ThemePreference, Tokens};

fn dump(preference: ThemePreference, system: Option<egui::Theme>) -> String {
    let mut h = Harness::builder().with_size(egui::vec2(1100.0, 800.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.set_theme_preference(preference);
        app
    });
    h.input_mut().system_theme = system;
    h.run_steps(4);
    format!(
        "tokens: {:#?}\n\nstyle_of(Light): {:#?}\n\nstyle_of(Dark): {:#?}\n",
        Tokens::get(&h.ctx),
        h.ctx.style_of(egui::Theme::Light),
        h.ctx.style_of(egui::Theme::Dark)
    )
}

#[test]
fn installed_styles_and_tokens_match_the_golden_files() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/theme");
    let update = std::env::var_os("UPDATE_GOLDEN").is_some();
    let mut failures = Vec::new();
    for (mode, preference) in [("system", ThemePreference::System), ("light", ThemePreference::Light), ("dark", ThemePreference::Dark)] {
        for (answer, system) in [("none", None), ("light", Some(egui::Theme::Light)), ("dark", Some(egui::Theme::Dark))] {
            let path = dir.join(format!("{mode}_{answer}.txt"));
            let actual = dump(preference, system);
            if update {
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(&path, &actual).unwrap();
            } else if std::fs::read_to_string(&path).ok().as_deref() != Some(actual.as_str()) {
                failures.push(path.display().to_string());
            }
        }
    }
    assert!(failures.is_empty(), "differs from the golden files (UPDATE_GOLDEN=1 regenerates): {failures:#?}");
}
