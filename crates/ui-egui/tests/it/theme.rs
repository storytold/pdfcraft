//! Theme preferences stay in sync across both menus, OS changes and saved settings. The mode a
//! frame resolved is read from `ui.state` (filled from the frame's resolved `RootEnv`), the saved
//! choice from the settings the app would write.

use craft::theme::ModeChoice;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;
use pdfcraft_ui_egui::settings::import_json;
use serde_json::json;

use crate::control::{ControlClient, answer, call, new_app, ok, send};

/// An app whose first frame has not run, restored from `settings` when given, on a desktop that
/// says `system`; the control channel is attached.
fn harness_with(settings: Option<&'static str>, system: Option<egui::Theme>, run: bool) -> (Harness<'static, PdfCraftApp>, ControlClient) {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut h = Harness::builder().with_size(egui::vec2(1100.0, 800.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            if let Some(json) = settings {
                app.apply_settings(&import_json(json).0);
            }
            app.attach_control(craft::automation::ControlChannel::new(rx), None);
            app
        }
    });
    os_theme(&mut h, system);
    if run {
        h.run_steps(4);
    }
    (h, tx)
}

/// The desktop reports `theme` as the windowing integration does: the raw input's field, and an
/// `Event::SystemTheme` for a scheme it knows. An integration never reports a known scheme as gone.
fn os_theme(h: &mut Harness<'static, PdfCraftApp>, theme: Option<egui::Theme>) {
    let input = h.input_mut();
    input.system_theme = theme;
    if let Some(theme) = theme {
        input.events.push(egui::Event::SystemTheme(theme));
    }
}

fn harness() -> (Harness<'static, PdfCraftApp>, ControlClient) {
    harness_with(None, Some(egui::Theme::Dark), true)
}

/// The choice the app would save now; a fresh install saves none, which is System.
fn saved_choice(h: &Harness<'static, PdfCraftApp>) -> ModeChoice {
    h.state().settings_snapshot().layout.theme.unwrap_or(ModeChoice::System)
}

/// The preference the app holds (`System`, `Light` or `Dark`) and the mode its last frame resolved
/// (`Light` or `Dark`), as `ui.state` reports them, and the choice the saved settings hold.
fn assert_theme(h: &mut Harness<'static, PdfCraftApp>, c: &ControlClient, preference: &str, mode: &str) {
    let state = ok(h, c, "ui.state", json!({}));
    assert_eq!(state["theme_preference"], preference);
    assert_eq!(state["theme"], mode);
    let choice = match preference {
        "Light" => ModeChoice::Light,
        "Dark" => ModeChoice::Dark,
        _ => ModeChoice::System,
    };
    assert_eq!(saved_choice(h), choice, "the saved choice follows the preference");
}

/// A click on the title bar's theme button cycles Light, Dark, System.
fn click_toolbar(h: &mut Harness<'static, PdfCraftApp>, label: &str) {
    // The button, not the tip that shows while the pointer is still over it from the last click.
    h.get_by_role_and_label(egui::accesskit::Role::Button, &format!("Display theme: {label}")).click();
    h.run_steps(3);
}

fn open_view_menu(h: &mut Harness<'static, PdfCraftApp>) {
    crate::support::open_menu(h, "View");
    assert!(h.query_by_label("Switch light / dark theme").is_none());
    crate::support::menu_row(h, "Display theme").hover();
    h.run_steps(3);
    for choice in ["Use system setting", "Light gray", "Dark gray"] {
        h.get_by_label(choice);
    }
}

/// A fresh install follows the desktop's appearance (#1248): dark on a dark desktop, light where
/// the desktop says nothing; a saved choice still wins.
#[test]
fn a_fresh_install_follows_the_system_theme() {
    let (mut h, c) = harness();
    assert_theme(&mut h, &c, "System", "Dark");
    let (mut quiet, qc) = harness_with(None, None, true);
    assert_theme(&mut quiet, &qc, "System", "Light");
    let (mut saved, sc) = harness_with(Some(r#"{"theme":"Light"}"#), Some(egui::Theme::Dark), true);
    assert_theme(&mut saved, &sc, "Light", "Light");
}

#[test]
fn toolbar_and_view_menu_share_three_state_preferences() {
    let (mut h, c) = harness();
    // Fresh, on a dark desktop: System, shown dark. One click on, explicit light wins even on a
    // dark desktop.
    assert_theme(&mut h, &c, "System", "Dark");
    click_toolbar(&mut h, "Use system setting");
    assert_theme(&mut h, &c, "Light", "Light");
    click_toolbar(&mut h, "Light gray");
    assert_theme(&mut h, &c, "Dark", "Dark");
    click_toolbar(&mut h, "Dark gray");
    assert_theme(&mut h, &c, "System", "Dark");

    open_view_menu(&mut h);
    h.get_by_label("Light gray").click();
    h.run_steps(3);
    assert_theme(&mut h, &c, "Light", "Light");
    click_toolbar(&mut h, "Light gray");
    assert_theme(&mut h, &c, "Dark", "Dark");

    open_view_menu(&mut h);
    h.get_by_label("Use system setting").click();
    h.run_steps(3);
    assert_theme(&mut h, &c, "System", "Dark");
    os_theme(&mut h, Some(egui::Theme::Light));
    h.run_steps(3);
    assert_theme(&mut h, &c, "System", "Light");
    click_toolbar(&mut h, "Use system setting");
    click_toolbar(&mut h, "Light gray");
    // A manual choice exits system mode and survives subsequent OS changes.
    os_theme(&mut h, Some(egui::Theme::Dark));
    h.run_steps(2);
    os_theme(&mut h, Some(egui::Theme::Light));
    h.run_steps(2);
    assert_theme(&mut h, &c, "Dark", "Dark");
}

/// The last release wrote `Light`, `Dark` and `System` capitalised and this one writes them in
/// lower case; both read, and a value that is neither leaves the system choice.
#[test]
fn saved_preferences_restore_with_legacy_light_dark_settings() {
    for (json, preference, mode) in [
        (r#"{"theme":"light"}"#, "Light", "Light"),
        (r#"{"theme":"dark"}"#, "Dark", "Dark"),
        (r#"{"theme":"system"}"#, "System", "Light"),
        (r#"{"theme":"Dark"}"#, "Dark", "Dark"),
        (r#"{"theme":"Light"}"#, "Light", "Light"),
        (r#"{"theme":"System"}"#, "System", "Light"),
        (r#"{"theme":"Purple"}"#, "System", "Light"),
    ] {
        let (mut h, c) = harness_with(Some(json), None, true);
        assert_theme(&mut h, &c, preference, mode);
    }
    // What one app saves, another reads back.
    let (mut h, c) = harness();
    ok(&mut h, &c, "ui.set", json!({"key": "theme", "value": "dark"}));
    let snapshot = h.state().settings_snapshot();
    let (mut restored, rc) = harness_with(None, None, true);
    restored.state_mut().apply_settings(&snapshot);
    restored.run_steps(2);
    assert_theme(&mut restored, &rc, "Dark", "Dark");
}

/// A theme command or option sent before the first frame takes effect on that frame.
#[test]
fn commands_and_options_apply_before_the_first_frame() {
    let (mut h, c) = harness_with(None, None, false);
    let steps = [
        ("ui.command", json!({"id": "view.theme.dark"}), "Dark", "Dark"),
        ("ui.command", json!({"id": "view.theme.system"}), "System", "Light"),
        // The palette/control toggle goes through the same preference.
        ("ui.command", json!({"id": "view.theme"}), "Dark", "Dark"),
        ("ui.command", json!({"id": "view.theme.light"}), "Light", "Light"),
        ("ui.set", json!({"key": "theme", "value": "dark"}), "Dark", "Dark"),
    ];
    let mut waiting = Vec::new();
    for (method, params, ..) in &steps {
        waiting.push(send(&c, method, params.clone()));
        waiting.push(send(&c, "ui.state", json!({})));
    }
    let refused = send(&c, "ui.set", json!({"key": "theme", "value": "purple"}));
    let after = send(&c, "ui.state", json!({}));
    h.step();
    for (pair, (.., preference, mode)) in waiting.chunks(2).zip(&steps) {
        answer(&pair[0]).unwrap();
        let state = answer(&pair[1]).unwrap();
        assert_eq!(state["theme_preference"], *preference);
        assert_eq!(state["theme"], *mode);
    }
    assert!(answer(&refused).unwrap_err().contains("light, dark, or system"));
    assert_eq!(answer(&after).unwrap()["theme_preference"], "Dark");
    // The first frame itself drew with it.
    assert_theme(&mut h, &c, "Dark", "Dark");
}

#[test]
fn restored_system_tracks_native_input_without_rewriting_saved_preference() {
    let (mut h, c) = harness_with(Some(r#"{"theme":"system"}"#), None, true);
    for (input, expected) in
        [(Some(egui::Theme::Dark), "Dark"), (Some(egui::Theme::Light), "Light"), (None, "Light"), (Some(egui::Theme::Dark), "Dark")]
    {
        os_theme(&mut h, input);
        h.run_steps(3);
        assert_theme(&mut h, &c, "System", expected);
    }
    for (manual, expected) in [("light", "Light"), ("dark", "Dark")] {
        ok(&mut h, &c, "ui.set", json!({"key": "theme", "value": manual}));
        for input in [Some(egui::Theme::Dark), Some(egui::Theme::Light), None] {
            os_theme(&mut h, input);
            h.run_steps(3);
            assert_theme(&mut h, &c, expected, expected);
        }
    }
    assert!(call(&mut h, &c, "ui.set", json!({"key": "theme", "value": "purple"})).is_err());
}

/// Preferences ▸ Appearance: manual Light/Dark mode choices stay fixed across OS changes,
/// Sync with system follows the OS both ways, and the Active badge tracks the active mode row.
#[test]
fn preferences_theme_cards_and_use_system_setting_stay_in_sync() {
    use egui::accesskit::Role;
    fn mode_shown(h: &Harness<'static, PdfCraftApp>) -> String {
        h.get_by_role_and_label(Role::ComboBox, "Appearance Mode").value().unwrap_or_default()
    }
    fn pick_mode(h: &mut Harness<'static, PdfCraftApp>, choice: &str) {
        h.get_by_role_and_label(Role::ComboBox, "Appearance Mode").scroll_to_me();
        h.run_steps(2);
        h.get_by_role_and_label(Role::ComboBox, "Appearance Mode").click();
        h.run_steps(3);
        h.get_all_by_label(choice).last().expect("mode option").click();
        h.run_steps(3);
    }
    let (mut h, c) = harness();
    h.get_by_label("Display theme: Use system setting").click_secondary();
    h.run_steps(4);
    h.get_by_label("Appearance settings…").click();
    h.run_steps(10);
    // A fresh install follows the OS (#1248), which is dark here.
    assert_theme(&mut h, &c, "System", "Dark");
    assert_eq!(mode_shown(&h), "Sync with system");
    assert_eq!(h.query_all_by_label("Active").count(), 1);

    // Choosing Dark locks Dark even though the OS is already dark.
    pick_mode(&mut h, "Dark");
    assert_theme(&mut h, &c, "Dark", "Dark");
    assert_eq!(mode_shown(&h), "Dark");

    // Choosing Light locks Light, and OS changes leave it alone.
    pick_mode(&mut h, "Light");
    os_theme(&mut h, Some(egui::Theme::Light));
    h.run_steps(2);
    os_theme(&mut h, Some(egui::Theme::Dark));
    h.run_steps(2);
    assert_theme(&mut h, &c, "Light", "Light");
    assert_eq!(mode_shown(&h), "Light");

    // Sync with system follows the OS both ways after a manual choice.
    pick_mode(&mut h, "Sync with system");
    assert_theme(&mut h, &c, "System", "Dark");
    assert_eq!(mode_shown(&h), "Sync with system");
    os_theme(&mut h, Some(egui::Theme::Light));
    h.run_steps(3);
    assert_theme(&mut h, &c, "System", "Light");
    // Only the Light row is marked Active while the OS is light.
    assert_eq!(h.query_all_by_label("Active").count(), 1);

    // Choosing Dark leaves system mode again.
    pick_mode(&mut h, "Dark");
    assert_theme(&mut h, &c, "Dark", "Dark");
    assert_eq!(mode_shown(&h), "Dark");
}
