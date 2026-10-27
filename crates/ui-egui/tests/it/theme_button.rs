//! The title bar's theme button: a click cycles System, Light and Dark (a fresh install starts
//! on System, #1248); a right-click opens the menu with the appearance settings. The Dark PDF
//! pages toggle sits beside it.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app
    });
    h.run_steps(6);
    h
}

fn preference(h: &Harness<'static, PdfCraftApp>) -> String {
    format!("{:?}", h.state().theme_preference)
}

#[test]
fn preferences_gear_is_right_of_theme_and_opens_preferences() {
    let mut h = harness();
    let theme = h.get_by_label("Display theme: Use system setting").rect();
    let gear = h.get_by_label("Preferences").rect();
    assert!(gear.left() >= theme.right());
    h.get_by_label("Preferences").click();
    h.run_steps(6);
    assert!(h.query_by_label("Appearance mode").is_some());
    assert!(h.query_by_label("Light theme").is_some());
    assert!(h.query_by_label("Dark theme").is_some());
    assert_eq!(preference(&h), "System");
}

#[test]
fn a_click_cycles_the_theme_and_a_right_click_opens_the_menu() {
    let mut h = harness();
    assert_eq!(preference(&h), "System", "a fresh install follows the desktop (#1248)");
    for (label, next) in
        [("Display theme: Use system setting", "Light"), ("Display theme: Light gray", "Dark"), ("Display theme: Dark gray", "System")]
    {
        h.get_by_label(label).click();
        h.run_steps(4);
        assert_eq!(preference(&h), next);
    }
    assert!(h.query_by_label("Appearance settings…").is_none(), "a plain click opens no menu");

    h.get_by_label("Display theme: Use system setting").click_secondary();
    h.run_steps(4);
    h.get_by_label("Appearance settings…").click();
    h.run_steps(4);
    h.run_steps(10);
    assert!(h.query_by_label("Light theme").is_some(), "Preferences opens on Appearance");
}

fn settle(h: &mut Harness<'static, PdfCraftApp>) {
    for _ in 0..200 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn dark_pdf_pages_toggle_is_beside_the_theme_button() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
        app.set_option("left", "closed").unwrap();
        app
    });
    h.run_steps(4);
    settle(&mut h);
    assert!(!h.state().views[0].dark_pages);
    h.get_by_label("Dark PDF pages").click();
    h.run_steps(3);
    assert!(h.state().views[0].dark_pages);
    assert!(h.state().view_defaults.dark_pages, "the choice carries over to newly opened documents");
    settle(&mut h);
    if let Ok(dir) = std::env::var("PDFCRAFT_SHOT") {
        h.render().unwrap().save(format!("{dir}/dark-pdf-pages.png")).unwrap();
    }
    h.get_by_label("Dark PDF pages").click();
    h.run_steps(3);
    assert!(!h.state().views[0].dark_pages);
}

/// The Dark PDF pages button's right-click menu picks the palette and what happens to images.
/// Keeping images (opaque ones by default) hands the dark page to the document's renderer;
/// recolouring them all filters the finished pixels. The choice applies to open documents,
/// carries over to new ones and survives a restart.
#[test]
fn dark_pages_menu_picks_the_palette_and_keeps_images() {
    use pdfcraft_render::{DarkImages, DarkPages, DarkPalette};
    let _gpu = crate::gpu();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
        app.set_option("left", "closed").unwrap();
        app
    });
    h.run_steps(4);
    settle(&mut h);
    let id = h.state().views[0].id;
    // A point on the page's paper, just inside its top-left corner.
    let paper = |h: &mut Harness<'static, PdfCraftApp>| {
        settle(h);
        let at = h.state().views[0].page_screen_rect(0).expect("page on screen").min + egui::vec2(4.0, 4.0);
        h.render().unwrap().get_pixel(at.x as u32, at.y as u32).0
    };
    let pick = |h: &mut Harness<'static, PdfCraftApp>, label: &str| {
        // A choice closes the menu: open it for each.
        h.get_by_label("Dark PDF pages").click_secondary();
        h.run_steps(4);
        h.get_by_label(label).click();
        h.run_steps(4);
    };
    assert_eq!(paper(&mut h), [255, 255, 255, 255]);
    h.get_by_label("Dark PDF pages").click();
    h.run_steps(3);
    pick(&mut h, "Black");
    let opaque = DarkPages { palette: DarkPalette::Black, images: DarkImages::KeepOpaque };
    assert_eq!((h.state().dark_style(), h.state().views[0].dark_style), (opaque, opaque), "opaque images are kept by default");
    assert_eq!(h.state().session.render_config(id).unwrap().dark_reading, opaque.reading_colors(), "the renderer draws the dark page");
    assert_eq!(paper(&mut h), [0, 0, 0, 255], "drawn by the renderer in the palette's paper");
    pick(&mut h, "Recolour images");
    let recolour = DarkPages { palette: DarkPalette::Black, images: DarkImages::Recolour };
    assert_eq!(h.state().views[0].dark_style, recolour);
    assert_eq!(h.state().session.render_config(id).unwrap().dark_reading, Default::default(), "the renderer draws the file's colours");
    assert_eq!(paper(&mut h), [0, 0, 0, 255], "every pixel filtered into the black palette");
    pick(&mut h, "Keep all images");
    let all = DarkPages { palette: DarkPalette::Black, images: DarkImages::KeepAll };
    assert_eq!(h.state().session.render_config(id).unwrap().dark_reading, all.reading_colors());
    assert_eq!(paper(&mut h), [0, 0, 0, 255]);
    // Off: the renderer goes back to the file's colours.
    h.get_by_label("Dark PDF pages").click();
    h.run_steps(3);
    assert_eq!(paper(&mut h), [255, 255, 255, 255]);
    assert_eq!(h.state().session.render_config(id).unwrap().dark_reading, Default::default());
    let mut restored = PdfCraftApp::new();
    restored.restore(&h.state().persist());
    assert_eq!(restored.dark_style(), all, "the style survives a restart");
}
