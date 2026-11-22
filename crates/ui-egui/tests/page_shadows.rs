//! Decorative page shadows are app preferences, never PDF edits.

use egui_kittest::{Harness, kittest::Queryable};
use pdfcraft_ui_egui::PdfCraftApp;
use pdfcraft_ui_egui::settings::{PdfCraftSettings, import_json};

/// Whether some pixel of the viewport outside every page rectangle differs between the two frames:
/// the shadow band round the pages.
fn differs_outside_pages(on: &image::RgbaImage, off: &image::RgbaImage, viewport: egui::Rect, pages: &[egui::Rect]) -> bool {
    let (right, bottom) = (viewport.right().floor() as u32, viewport.bottom().floor() as u32);
    (viewport.top().ceil() as u32..bottom.min(on.height())).any(|y| {
        (viewport.left().ceil() as u32..right.min(on.width())).any(|x| {
            let at = egui::pos2(x as f32, y as f32);
            !pages.iter().any(|page| page.contains(at)) && on.get_pixel(x, y) != off.get_pixel(x, y)
        })
    })
}

#[test]
fn viewer_shadows_follow_the_preference_without_moving_pages() {
    for theme in ["light", "dark"] {
        for layout in ["single", "two-page", "continuous", "two-up"] {
            let mut h = harness(theme);
            h.set_option("layout", layout).unwrap();
            h.run_steps(3);
            let rectangles = h.state().views[0].visible_page_rects().to_vec();
            let pages: Vec<_> = rectangles.iter().map(|(_, rect)| *rect).collect();
            let viewport = h.state().views[0].viewport_rect();
            let on = h.render().unwrap();
            h.set_option("show_page_edge_shadows", "false").unwrap();
            h.run_steps(2);
            let off = h.render().unwrap();
            assert_eq!(h.state().views[0].visible_page_rects(), rectangles, "{theme} {layout}: pages stay where they are");
            assert!(differs_outside_pages(&on, &off, viewport, &pages), "{theme} {layout}: the shadow is drawn only while the preference is on");
            h.set_option("show_page_edge_shadows", "true").unwrap();
            h.run_steps(2);
            let back = h.render().unwrap();
            assert!(!differs_outside_pages(&on, &back, viewport, &pages), "{theme} {layout}: turning it on again restores the shadow");
        }
    }
}

#[test]
fn checkbox_and_multiple_documents_do_not_edit_pdf_state() {
    let mut h = harness("light");
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "second.pdf", None, pdf()).unwrap();
    let ids: Vec<_> = h.state().views.iter().map(|v| v.id).collect();
    h.state_mut().session.apply(ids[0], pdfcraft_engine::Edit::SetInfo { key: "Title".into(), value: "Keep existing edits".into() }).unwrap();
    assert!(h.state().session.get(ids[0]).unwrap().dirty);
    assert!(h.state().session.get(ids[0]).unwrap().can_undo().is_some());
    let saved: Vec<_> = ids.iter().map(|id| h.state_mut().session.save_bytes(*id).unwrap()).collect();
    let before: Vec<_> = h
        .state()
        .views
        .iter()
        .map(|v| {
            let d = h.state().session.get(v.id).unwrap();
            (
                d.bytes.clone(),
                d.dirty,
                d.can_undo().map(str::to_owned),
                d.can_redo().map(str::to_owned),
                d.info.pages.iter().map(|p| (p.width, p.height, p.rotation)).collect::<Vec<_>>(),
            )
        })
        .collect();
    h.set_option("dialog", "preferences").unwrap();
    h.run_steps(3);
    assert_eq!(h.state().settings_snapshot().behavior.show_page_edge_shadows, Some(true));
    // Preferences is one scrolling page; the shadows are in the Appearance card.
    h.get_by_label("Show Page Edge Shadows").scroll_to_me();
    h.run_steps(3);
    let checkbox = h.get_by_label("Show Page Edge Shadows");
    checkbox.click();
    h.run_steps(2);
    // The preference applies as the row changes.
    assert_eq!(h.state().settings_snapshot().behavior.show_page_edge_shadows, Some(false), "a click turns it off");
    h.get_by_label("Show Page Edge Shadows").focus();
    h.key_press(egui::Key::Space);
    h.run_steps(2);
    assert_eq!(h.state().settings_snapshot().behavior.show_page_edge_shadows, Some(true), "Space turns it on");
    h.set_option("show_page_edge_shadows", "false").unwrap();
    h.run_steps(2);
    h.set_option("dialog", "none").unwrap();
    for tab in [0, 1] {
        h.state_mut().active = Some(tab);
        h.run_steps(2);
        let d = h.state().session.get(h.state().views[tab].id).unwrap();
        assert_eq!(
            (&d.bytes, d.dirty, d.can_undo(), d.can_redo()),
            (&before[tab].0, before[tab].1, before[tab].2.as_deref(), before[tab].3.as_deref())
        );
        assert_eq!(d.info.pages.iter().map(|p| (p.width, p.height, p.rotation)).collect::<Vec<_>>(), before[tab].4);
    }
    for (id, bytes) in ids.iter().zip(&saved) {
        assert_eq!(h.state_mut().session.save_bytes(*id).unwrap(), *bytes);
    }
}

#[test]
fn rendered_page_content_is_identical_with_and_without_decoration() {
    let mut h = harness("dark");
    h.set_option("panel", "pages").unwrap();
    h.set_option("zoom", "175").unwrap();
    h.run_steps(3);
    for _ in 0..400 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(!h.state().render_pending(), "the actual PDF raster must be ready");
    let rects = h.state().views[0].visible_page_rects().to_vec();
    let enabled = h.render().unwrap();
    h.set_option("show_page_edge_shadows", "false").unwrap();
    h.run_steps(2);
    let disabled = h.render().unwrap();
    assert_eq!(h.state().views[0].visible_page_rects(), rects);
    let viewport = h.state().views[0].viewport_rect();
    // Only the shadow outside the pages may change: artwork, annotation and form pixels inside
    // each page are identical.
    for (_, page) in &rects {
        let visible = page.intersect(viewport).shrink(2.0);
        if !visible.is_positive() {
            continue;
        }
        for y in visible.top().ceil() as u32..visible.bottom().floor() as u32 {
            for x in visible.left().ceil() as u32..visible.right().floor() as u32 {
                assert_eq!(enabled.get_pixel(x, y), disabled.get_pixel(x, y), "page content at {x},{y}");
            }
        }
    }
    // The Pages sidebar, including its thumbnails and selection borders, is unaffected.
    for y in 0..enabled.height() {
        for x in (0..viewport.left().floor() as u32).chain(viewport.right().ceil() as u32..enabled.width()) {
            assert_eq!(enabled.get_pixel(x, y), disabled.get_pixel(x, y), "sidebar at {x},{y}");
        }
    }
    assert_ne!(enabled.as_raw(), disabled.as_raw(), "the decoration really changes the image");
    if let Ok(directory) = std::env::var("PDFCRAFT_SHADOW_SCREENSHOTS") {
        enabled.save(format!("{directory}/page-shadows-enabled.png")).unwrap();
        disabled.save(format!("{directory}/page-shadows-disabled.png")).unwrap();
    }
}

fn harness(theme: &str) -> Harness<'static, PdfCraftApp> {
    let theme = theme.to_string();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        install_runtime(&cc.egui_ctx);
        {
            let mut app = new_app();
            // Save comparisons must not depend on crossing a /ModDate second boundary.
            app.session = app.session.with_clock(|| 1_700_000_000);
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.set_option(&egui::Context::default(), "theme", &theme).unwrap();
            app.open_bytes(&egui::Context::default(), "pages.pdf", None, pdf()).unwrap();
            app.set_option(&egui::Context::default(), "zoom", "100").unwrap();
            app
        }
    });
    h.run_steps(3);
    h
}

/// Synthetic pages include colored artwork, an annotation and a form field.
fn pdf() -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [7 0 R] >> >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 300 400] >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /Contents 8 0 R /Annots [6 0 R 7 0 R] >>".to_string(),
        "<< /Type /Page /Parent 2 0 R >>".to_string(),
        "<< /Type /Page /Parent 2 0 R >>".to_string(),
        "<< /Type /Annot /Subtype /Text /Rect [20 20 40 40] /Contents (Note) >>".to_string(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (field) /V (value) /Rect [50 50 100 70] /P 3 0 R >>".to_string(),
        {
            let body = "0.8 0.9 1 rg 0 0 300 400 re f 0.3 g 10 10 280 8 re f";
            format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len())
        },
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    bytes
}

#[test]
fn page_shadow_preference_round_trips_and_ignores_invalid_settings() {
    let mut app = new_app();
    let enabled = |app: &PdfCraftApp| app.settings_snapshot().behavior.show_page_edge_shadows;
    let restore_json = |app: &mut PdfCraftApp, json: &str| app.apply_settings(&import_json(json).0);
    assert_eq!(enabled(&app), Some(true));
    app.apply_settings(&PdfCraftSettings::default());
    assert_eq!(enabled(&app), Some(true));
    for value in [false, true] {
        app.set_option(&egui::Context::default(), "show_page_edge_shadows", if value { "true" } else { "false" }).unwrap();
        let mut restored = new_app();
        restored.apply_settings(&app.settings_snapshot());
        assert_eq!(enabled(&restored), Some(value));
    }
    for invalid in ["null", "0", "\"false\"", "[]", "{}"] {
        restore_json(&mut app, &format!("{{\"show_page_edge_shadows\":{invalid}}}"));
        assert_eq!(enabled(&app), Some(true));
    }
    restore_json(&mut app, "not json");
    assert_eq!(enabled(&app), Some(true));
    assert!(app.set_option(&egui::Context::default(), "show_page_edge_shadows", "maybe").is_err());
    assert_eq!(enabled(&app), Some(true));
}

/// An app on the default host with test services: no wake-ups, no autosave folder, the GitHub update
/// source, a fresh intake and an inline watch (as `tests/it/control.rs`' `new_app`).
fn new_app() -> pdfcraft_ui_egui::PdfCraftApp {
    use craft::host::{AppInfo, Host, Wake};
    use craft::platform::clipboard::{Clipboard, SystemClipboard};
    use craft::platform::open::FileIntake;
    use craft::platform::watch::FileWatch;
    let host = Host::builder(AppInfo { id: "pdfcraft", name: "PdfCraft", version: "test" }).build();
    let services = pdfcraft_ui_egui::AppServices::new(
        Wake::none(),
        Clipboard::new(SystemClipboard::new()),
        None,
        None,
        FileIntake::new(Wake::none()).0,
        FileWatch::new(Wake::none()).inline(),
    );
    pdfcraft_ui_egui::PdfCraftApp::with_host(std::sync::Arc::new(host), services)
}

/// The app's command and option entry points with the harness's own context (L-208).
trait AppHarness {
    fn execute(&mut self, id: &str) -> bool;
    fn set_option(&mut self, key: &str, value: &str) -> Result<(), String>;
}

impl AppHarness for egui_kittest::Harness<'_, pdfcraft_ui_egui::PdfCraftApp> {
    fn execute(&mut self, id: &str) -> bool {
        let ctx = self.ctx.clone();
        self.state_mut().execute(&ctx, id)
    }

    fn set_option(&mut self, key: &str, value: &str) -> Result<(), String> {
        let ctx = self.ctx.clone();
        self.state_mut().set_option(&ctx, key, value)
    }
}

/// Installs the framework runtime on a harness context, as `tests/it/support`'s `install_runtime` (L-227).
fn install_runtime(ctx: &egui::Context) {
    let spec = craft::ui::InstallSpec::new(craft::host::target::ShellInfo::default(), std::sync::Arc::new(craft::window::system::NoWindows));
    craft::ui::shell::embed::install(ctx, spec, &pdfcraft_ui_egui::fonts::ui_fonts(pdfcraft_ui_egui::i18n::Lang::EN))
        .expect("the runtime installs on a fresh context");
}
