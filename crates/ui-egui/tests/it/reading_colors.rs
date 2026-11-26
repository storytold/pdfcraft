//! Preferences ▸ Reading colours: the app-wide preference (Acrobat's Replace Document Colors),
//! how it survives a restart, and how documents open with it.

use crate::support::AppHarness as _;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_render::ReadingColors;
use pdfcraft_ui_egui::PdfCraftApp;

/// One valid page (proper xref, so no repair banner covers the dialog): black text on white.
fn pdf() -> Vec<u8> {
    crate::support::fixtures::pdf_pages(1)
}

#[test]
fn pending_pages_use_document_reading_background() {
    use egui::{Color32, Context, RawInput, Rect, Shape, pos2, vec2};

    let custom = ReadingColors::parse("custom", Some("#eeeeee"), Some("#182430"), None, None).unwrap();
    for (colors, expected) in [
        (ReadingColors::default(), Color32::WHITE),
        (ReadingColors::BLACK_ON_WHITE, Color32::WHITE),
        (ReadingColors::WHITE_ON_BLACK, Color32::BLACK),
        (ReadingColors::YELLOW_ON_BLACK, Color32::BLACK),
        (ReadingColors::GREEN_ON_BLACK, Color32::BLACK),
        (custom, Color32::from_rgb(0x18, 0x24, 0x30)),
    ] {
        let (mut app, _jobs) = crate::control::app_with_manual_jobs();
        app.set_option(&egui::Context::default(), "language", "en").unwrap();
        app.open_bytes(&egui::Context::default(), "pending.pdf", None, crate::support::fixtures::pdf_pages(3)).unwrap();
        let id = app.views[0].id;
        // Deliberately differ from the app preference: the document is the source of truth.
        app.session.set_reading_colors(id, colors);
        // The page view runs on the framework's runtime, which knows its commands' scopes.
        let ctx = Context::default();
        crate::support::install_runtime(&ctx);
        for page in 0..3 {
            app.views[0].go_to_page(page);
            // Draw the real canvas without finishing the render frame, so no texture can
            // race the assertion or hide the pending-page background.
            let mut raw = RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 600.0))), ..Default::default() };
            <PdfCraftApp as eframe::App>::raw_input_hook(&mut app, &ctx, &mut raw);
            let mut textures = usize::MAX;
            let mut output = ctx.run_ui(raw, |ui| {
                let rect = ui.max_rect();
                textures = pdfcraft_ui_egui::canvas::document_area_at(&mut app, 0, rect, ui).expect("the page view runs").textures.entries;
            });
            output.textures_delta.clear();
            let view = &app.views[0];
            assert_eq!(textures, 0, "the page must still be waiting for its picture");
            let rect = view.page_screen_rect(page).expect("target page visible");
            fn find_fill(shape: &Shape, rect: Rect) -> Option<Color32> {
                match shape {
                    Shape::Rect(r) if r.rect == rect && r.stroke.width == 0.0 => Some(r.fill),
                    Shape::Mesh(mesh) if !mesh.vertices.is_empty() => {
                        let bounds = mesh.calc_bounds();
                        let matches = (bounds.min.x - rect.min.x).abs() <= 2.0
                            && (bounds.min.y - rect.min.y).abs() <= 2.0
                            && (bounds.max.x - rect.max.x).abs() <= 2.0
                            && (bounds.max.y - rect.max.y).abs() <= 2.0;
                        matches.then(|| mesh.vertices.iter().find(|v| v.color.a() == 255).map(|v| v.color)).flatten()
                    }
                    Shape::Vec(shapes) => shapes.iter().find_map(|child| find_fill(child, rect)),
                    _ => None,
                }
            }
            let fill = output.shapes.iter().find_map(|s| find_fill(&s.shape, rect));
            assert_eq!(fill, Some(expected), "page {page}, reading mode {}", colors.mode_name());
        }
    }
}

/// Picks `option` in the Appearance page's Reading colours row, a select whose options show once it is opened.
fn pick_reading_colours(h: &mut Harness<'static, PdfCraftApp>, option: &str) {
    h.get_by_role_and_label(egui::accesskit::Role::ComboBox, "Reading colours").scroll_to_me();
    h.run_steps(3);
    h.get_by_role_and_label(egui::accesskit::Role::ComboBox, "Reading colours").click();
    h.run_steps(3);
    h.get_all_by_label(option).last().unwrap_or_else(|| panic!("the {option:?} option is on screen")).click();
    h.run_steps(3);
}

/// A small PNG for the Create ▸ Images path, which opens the document it builds.
fn png() -> Vec<u8> {
    let img = craft_image_codecs::Image::from_u8(8, 4, craft_image_codecs::ChannelLayout::Rgb, vec![120u8; 8 * 4 * 3]).unwrap();
    craft_image_codecs::encode(&img, craft_image_codecs::Format::Png, &craft_image_codecs::EncodeOptions::default()).unwrap()
}

#[test]
fn reading_colours_preference_round_trips() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "doc.pdf", None, pdf()).expect("opens");
            app.set_option(&egui::Context::default(), "dialog", "preferences").unwrap();
            app
        }
    });
    h.run_steps(4);
    assert_eq!(h.state().reading_colors, ReadingColors::default());
    pick_reading_colours(&mut h, "White on black");
    assert_eq!(h.state().reading_colors, ReadingColors::WHITE_ON_BLACK, "the row applies immediately");
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(h.state().reading_colors, ReadingColors::WHITE_ON_BLACK);
    // The preference is app-wide: the already open document follows it too.
    let id = h.state().views[0].id;
    assert_eq!(h.state().session.reading_colors(id), Some(ReadingColors::WHITE_ON_BLACK));
    // Closing the dialog keeps everything.
    h.set_option("dialog", "none").unwrap();
    h.run_steps(1);

    // The choice survives a restart through the same path the settings file uses.
    let mut restored = crate::control::new_app();
    restored.apply_settings(&h.state().settings_snapshot());
    assert_eq!(restored.reading_colors, ReadingColors::WHITE_ON_BLACK);
    // A settings file without the field keeps Off.
    let restore = |app: &mut PdfCraftApp, json: &str| app.apply_settings(&pdfcraft_ui_egui::settings::import_json(json).0);
    let mut fresh = crate::control::new_app();
    restore(&mut fresh, r#"{"theme": "dark"}"#);
    assert_eq!(fresh.reading_colors, ReadingColors::default());
    // A corrupt value falls back to Off instead of panicking.
    restore(&mut fresh, r#"{"reading_colors": {"params": {"text": "zzz", "background": [0, 0, 0], "only_black": false, "line_art": true}}}"#);
    assert_eq!(fresh.reading_colors, ReadingColors::default());
    restore(&mut fresh, r#"{"reading_colors": 7}"#);
    assert_eq!(fresh.reading_colors, ReadingColors::default());
}

#[test]
fn reading_presets_follow_appearance_and_do_not_change_the_interface_theme() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1000.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.set_option(&egui::Context::default(), "dialog", "preferences").unwrap();
            app
        }
    });
    h.run_steps(6);
    // The document's own colours: the row starts on Off.
    let row = h.get_by_role_and_label(egui::accesskit::Role::ComboBox, "Reading colours");
    assert_eq!(row.value().as_deref(), Some("Off"));
    let theme = h.state().theme_preference;
    for (label, expected) in [
        ("White on black", ReadingColors::WHITE_ON_BLACK),
        ("Yellow on black", ReadingColors::YELLOW_ON_BLACK),
        ("Green on black", ReadingColors::GREEN_ON_BLACK),
        ("Black on white", ReadingColors::BLACK_ON_WHITE),
        ("Off", ReadingColors::default()),
    ] {
        pick_reading_colours(&mut h, label);
        h.get_by_label("OK").click();
        h.run_steps(3);
        assert_eq!(h.state().reading_colors, expected);
        assert_eq!(h.state().theme_preference, theme);
        h.state_mut().open_preferences();
        h.run_steps(4);
    }
    if let Ok(path) = std::env::var("PDFCRAFT_READING_SHOT") {
        h.render().unwrap().save(path).unwrap();
    }
}

#[test]
fn reading_colours_seed_on_open() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.reading_colors = ReadingColors::WHITE_ON_BLACK;
            app.open_bytes(&egui::Context::default(), "doc.pdf", None, pdf()).expect("opens");
            app
        }
    });
    h.run_steps(4);
    // The document opened with the preference already applied.
    let id = h.state().views[0].id;
    assert_eq!(h.state().session.reading_colors(id), Some(ReadingColors::WHITE_ON_BLACK));
    // Changing the preference applies it to the open document as well.
    h.state_mut().set_reading_colors(ReadingColors::YELLOW_ON_BLACK);
    h.run_steps(1);
    assert_eq!(h.state().session.reading_colors(id), Some(ReadingColors::YELLOW_ON_BLACK));

    // Revisions and the signed version open through their own session calls, bypassing the
    // ordinary open: they seed the new document the same way. A second revision comes from an
    // incremental save.
    {
        let s = &mut h.state_mut().session;
        s.apply(id, pdfcraft_engine::Edit::SetInfo { key: "Title".into(), value: "Second".into() }).unwrap();
        let saved = s.save_bytes(id).unwrap();
        s.mark_saved(id, saved, None).unwrap();
    }
    h.state_mut().open_revision(1);
    let revision = h.state().views.last().unwrap().id;
    assert_ne!(revision, id);
    assert_eq!(h.state().session.reading_colors(revision), Some(ReadingColors::YELLOW_ON_BLACK));
    let len = h.state().session.get(id).unwrap().bytes.len();
    h.state_mut().view_signed_version(len, 1);
    let signed = h.state().views.last().unwrap().id;
    assert_ne!(signed, id);
    assert_eq!(h.state().session.reading_colors(signed), Some(ReadingColors::YELLOW_ON_BLACK));

    // Created documents open through their own session calls too — a comment summary via
    // `open_created`, an image import via `open_new_document` — and seed the same way.
    h.state_mut().summarize_comments();
    let summary = h.state().views.last().unwrap().id;
    assert_ne!(summary, signed);
    assert_eq!(h.state().session.reading_colors(summary), Some(ReadingColors::YELLOW_ON_BLACK));
    h.state_mut().create_from_images(vec![("scan.png".into(), png())]);
    let created = h.state().views.last().unwrap().id;
    assert_ne!(created, summary);
    assert_eq!(h.state().session.reading_colors(created), Some(ReadingColors::YELLOW_ON_BLACK));

    // And back off, for every open document.
    h.state_mut().set_reading_colors(ReadingColors::default());
    h.run_steps(1);
    for view in &h.state().views {
        assert_eq!(h.state().session.reading_colors(view.id), Some(ReadingColors::default()));
    }
}
