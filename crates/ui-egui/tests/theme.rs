//! Theme preferences stay in sync across both menus, OS changes and saved settings.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;
use pdfcraft_ui_egui::theme::{ThemeKind, ThemePreference, Tokens};

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1100.0, 800.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app
    });
    h.input_mut().system_theme = Some(egui::Theme::Dark);
    h.run_steps(4);
    h
}

fn assert_theme(h: &Harness<'static, PdfCraftApp>, preference: ThemePreference, kind: ThemeKind) {
    assert_eq!(h.state().theme_preference, preference);
    assert_eq!(h.state().theme, kind);
    assert_eq!(Tokens::get(&h.ctx).kind, kind);
    assert_eq!(h.ctx.global_style().visuals.dark_mode, kind == ThemeKind::Dark);
    assert_eq!(h.ctx.theme(), if kind == ThemeKind::Dark { egui::Theme::Dark } else { egui::Theme::Light });
}

fn open_toolbar(h: &mut Harness<'static, PdfCraftApp>, label: &str) {
    h.get_by_label(&format!("Display theme: {label}")).click();
    h.run_steps(3);
    for choice in ["Use system setting", "Light gray", "Dark gray"] {
        h.get_by_label(choice);
    }
}

fn open_view_menu(h: &mut Harness<'static, PdfCraftApp>) {
    h.get_by_label("Menu").click();
    h.run_steps(2);
    h.get_by_label("View ⏵").hover();
    h.run_steps(3);
    assert!(h.query_by_label("Switch light / dark theme").is_none());
    h.get_by_label("Display theme ⏵").hover();
    h.run_steps(3);
    for choice in ["Use system setting", "Light gray", "Dark gray"] {
        h.get_by_label(choice);
    }
}

#[test]
fn toolbar_and_view_menu_share_three_state_preferences() {
    let mut h = harness();
    // Explicit light wins even on a dark desktop, including egui's popup style.
    assert_theme(&h, ThemePreference::Light, ThemeKind::Light);
    open_toolbar(&mut h, "Light gray");
    h.get_by_label("Use system setting").click();
    h.run_steps(3);
    assert_theme(&h, ThemePreference::System, ThemeKind::Dark);

    open_view_menu(&mut h);
    h.get_by_label("Light gray").click();
    h.run_steps(3);
    assert_theme(&h, ThemePreference::Light, ThemeKind::Light);
    open_toolbar(&mut h, "Light gray");
    h.get_by_label("Dark gray").click();
    h.run_steps(3);
    assert_theme(&h, ThemePreference::Dark, ThemeKind::Dark);

    open_view_menu(&mut h);
    h.get_by_label("Use system setting").click();
    h.run_steps(3);
    assert_theme(&h, ThemePreference::System, ThemeKind::Dark);
    h.input_mut().system_theme = Some(egui::Theme::Light);
    h.run_steps(3);
    assert_theme(&h, ThemePreference::System, ThemeKind::Light);
    open_toolbar(&mut h, "Use system setting");
    h.get_by_label("Dark gray").click();
    h.run_steps(3);
    // A manual choice exits system mode and survives subsequent OS changes.
    h.input_mut().system_theme = Some(egui::Theme::Dark);
    h.run_steps(2);
    h.input_mut().system_theme = Some(egui::Theme::Light);
    h.run_steps(2);
    assert_theme(&h, ThemePreference::Dark, ThemeKind::Dark);
}

#[test]
fn saved_preferences_restore_with_legacy_light_dark_settings() {
    for (value, preference) in [("light", ThemePreference::Light), ("dark", ThemePreference::Dark), ("system", ThemePreference::System)] {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.set_option("theme", value).unwrap();
        let saved = app.persist();
        let mut restored = PdfCraftApp::new();
        restored.set_option("language", "en").unwrap();
        restored.restore(&saved);
        assert_eq!(restored.theme_preference, preference);
    }
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    app.restore(r#"{"theme":"Dark"}"#);
    assert_eq!(app.theme_preference, ThemePreference::Dark);
    assert_eq!(app.theme, ThemeKind::Dark);
    app.restore(r#"{"theme":"Light"}"#);
    assert_eq!(app.theme_preference, ThemePreference::Light);
    assert_eq!(app.theme, ThemeKind::Light);
    app.restore(r#"{"theme":"System"}"#);
    app.restore(r#"{"theme":"Purple"}"#);
    assert_eq!(app.theme_preference, ThemePreference::System);
}

#[test]
fn commands_and_options_apply_before_the_first_frame() {
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    app.run_command("view.theme.dark");
    assert_eq!(app.theme_preference, ThemePreference::Dark);
    assert_eq!(app.theme, ThemeKind::Dark);
    app.run_command("view.theme.system");
    assert_eq!(app.theme_preference, ThemePreference::System);
    // The existing palette/control toggle also goes through the preference setter.
    app.run_command("view.theme");
    assert_eq!(app.theme_preference, ThemePreference::Light);
    assert_eq!(app.theme, ThemeKind::Light);
    app.run_command("view.theme.light");
    app.set_option("theme", "dark").unwrap();
    assert!(app.set_option("theme", "purple").unwrap_err().contains("light, dark, or system"));
    assert_eq!(app.theme_preference, ThemePreference::Dark);
    assert_eq!(app.theme, ThemeKind::Dark);
}
