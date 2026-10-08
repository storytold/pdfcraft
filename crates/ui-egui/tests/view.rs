//! Page layouts, zoom modes and link navigation, checked by where pages land on screen.

use egui::{Key, Modifiers};
use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

/// Three 300×400 pt pages. Page 1 links to page 3.
const PAGES: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /Annots [6 0 R] >> endobj
4 0 obj << /Type /Page /Parent 2 0 R >> endobj
5 0 obj << /Type /Page /Parent 2 0 R >> endobj
6 0 obj << /Type /Annot /Subtype /Link /Rect [50 300 250 350] /Border [0 0 0] /Dest [5 0 R /Fit] >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// Tests that render pixels (`h.render()`) take this first, so only one wgpu device compiles
/// shaders at a time. On machines without a GPU, wgpu falls back to Microsoft's WARP, whose ARM64
/// pixel-shader JIT crashes (access violation in `d3d10warp!PixelJitProgram::ClassifyVars`) when
/// two devices compile at once, as on GitHub's Windows 11 ARM64 runner. Hold it for the whole
/// test (declared before the harness, so it's released after the device is dropped).
static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn gpu() -> std::sync::MutexGuard<'static, ()> {
    // A test that panicked while holding it leaves nothing to clean up.
    GPU.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn harness(options: &'static [(&'static str, &'static str)]) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("pages.pdf", None, PAGES.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "none").unwrap();
        for (k, v) in options {
            app.set_option(k, v).unwrap();
        }
        app
    });
    h.run_steps(8);
    h
}

fn rect(h: &Harness<'static, PdfCraftApp>, page: usize) -> Option<egui::Rect> {
    h.state().views[0].page_screen_rect(page)
}

#[cfg(target_os = "linux")]
#[test]
fn middle_button_scrolling_keeps_page_colours_in_both_themes() {
    use egui_kittest::kittest::Queryable;
    let _gpu = gpu();
    for theme in ["light", "dark"] {
        for organize in [false, true] {
            let mut h = harness(&[("zoom", "50")]);
            h.state_mut().set_option("theme", theme).unwrap();
            h.state_mut().set_option("organize", if organize { "on" } else { "off" }).unwrap();
            for _ in 0..100 {
                h.run_steps(2);
                if !h.state().render_pending() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let page = if organize { h.get_by_label("Page 1").rect() } else { rect(&h, 0).unwrap() };
            let at = page.center();
            let ppp = h.ctx.pixels_per_point();
            let before = *h.render().unwrap().get_pixel((at.x * ppp) as u32, (at.y * ppp) as u32);
            assert_eq!(before, image::Rgba([255, 255, 255, 255]), "the synthetic page is white");
            let anchor = h.state().views[0].viewport_rect().center();
            h.event(egui::Event::PointerMoved(anchor));
            h.event(egui::Event::PointerButton { pos: anchor, button: egui::PointerButton::Middle, pressed: true, modifiers: Modifiers::NONE });
            h.run_steps(1);
            h.event(egui::Event::PointerButton { pos: anchor, button: egui::PointerButton::Middle, pressed: false, modifiers: Modifiers::NONE });
            h.run_steps(1);
            assert!(h.state().views[0].auto_scrolling());
            let during = *h.render().unwrap().get_pixel((at.x * ppp) as u32, (at.y * ppp) as u32);
            assert_eq!(during, before, "scrolling preserves page colours: theme={theme}, organize={organize}");
        }
    }
}

#[test]
fn continuous_layout_stacks_pages_vertically() {
    let mut h = harness(&[("layout", "continuous"), ("zoom", "50")]);
    h.run_steps(4);
    let (a, b) = (rect(&h, 0).expect("page 1 on screen"), rect(&h, 1).expect("page 2 on screen"));
    assert!(b.min.y > a.max.y, "page 2 below page 1: {a:?} {b:?}");
    assert!((a.center().x - b.center().x).abs() < 1.0, "same column");
}

#[test]
fn two_up_layout_places_pages_side_by_side() {
    let mut h = harness(&[("layout", "two-up"), ("zoom", "50")]);
    h.run_steps(4);
    let (a, b) = (rect(&h, 0).expect("page 1"), rect(&h, 1).expect("page 2"));
    assert!((a.min.y - b.min.y).abs() < 1.0, "same row: {a:?} {b:?}");
    assert!(b.min.x > a.max.x, "page 2 right of page 1");
    let c = rect(&h, 2).expect("page 3 on the next row");
    assert!(c.min.y > a.max.y);
}

#[test]
fn single_page_layout_shows_one_page_at_a_time() {
    let mut h = harness(&[("layout", "single"), ("zoom", "50")]);
    h.run_steps(4);
    assert!(rect(&h, 0).is_some() && rect(&h, 1).is_none(), "only page 1");
    h.state_mut().set_option("page", "2").unwrap();
    h.run_steps(4);
    assert!(rect(&h, 0).is_none() && rect(&h, 1).is_some(), "only page 2");
}

#[test]
fn actual_size_fit_width_and_fit_page() {
    let mut h = harness(&[]);
    let pt = 96.0 / 72.0;

    h.key_press_modifiers(Modifiers::COMMAND, Key::Num1); // actual size
    h.run_steps(4);
    let r = rect(&h, 0).expect("page 1");
    assert!((r.width() - 300.0 * pt).abs() < 1.0, "100% is 300 pt at 96 dpi: {}", r.width());

    h.key_press_modifiers(Modifiers::COMMAND, Key::Num2); // fit width
    h.run_steps(4);
    let (r, vp) = (rect(&h, 0).expect("page 1"), h.state().views[0].viewport_rect());
    assert!(r.width() > vp.width() * 0.85 && r.width() <= vp.width(), "fit width: page {} in viewport {}", r.width(), vp.width());

    h.key_press_modifiers(Modifiers::COMMAND, Key::Num0); // fit page
    h.run_steps(4);
    let (r, vp) = (rect(&h, 0).expect("page 1"), h.state().views[0].viewport_rect());
    assert!(r.height() <= vp.height() && r.height() > vp.height() * 0.85, "fit page: page {} in viewport {}", r.height(), vp.height());
    assert!(r.width() < vp.width(), "a portrait page fits by height");
}

#[test]
fn clicking_a_link_goes_to_its_destination() {
    // Default fit-width zoom: pages are taller than the window, so the target can scroll to the top.
    let mut h = harness(&[]);
    h.run_steps(4);
    let r = rect(&h, 0).expect("page 1");
    // The link spans x 50..250, y 300..350 in PDF space (y up) on a 300×400 page.
    let at = egui::pos2(r.min.x + r.width() * (150.0 / 300.0), r.min.y + r.height() * (1.0 - 325.0 / 400.0));
    h.hover_at(at);
    h.run_steps(2);
    h.drag_at(at); // press
    h.drop_at(at); // release
    h.run_steps(8);
    assert_eq!(h.state().views[0].current, 2, "the link targets page 3");
    let (target, vp) = (rect(&h, 2).expect("page 3 on screen"), h.state().views[0].viewport_rect());
    assert!((target.min.y - vp.min.y).abs() < 40.0, "page 3 is scrolled to the top: {target:?} in {vp:?}");
}

/// Two pages with a text field on each and a non-embedded Helvetica.
const FORM: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R 7 0 R] >> >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /Annots [6 0 R] /Contents 5 0 R /Resources << /Font << /F1 8 0 R >> >> >> endobj
4 0 obj << /Type /Page /Parent 2 0 R /Annots [7 0 R] >> endobj
5 0 obj << /Length 37 >> stream
BT /F1 12 Tf 20 20 Td (Form) Tj ET
endstream endobj
6 0 obj << /Type /Annot /Subtype /Widget /FT /Tx /T (fullname) /V (Ada) /Rect [50 300 250 330] /P 3 0 R >> endobj
7 0 obj << /Type /Annot /Subtype /Widget /FT /Tx /T (postcode) /Rect [50 300 250 330] /P 4 0 R >> endobj
8 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj
trailer << /Root 1 0 R >>
%%EOF";

fn form_harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("form.pdf", None, FORM.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app
    });
    h.run_steps(8);
    h
}

#[test]
fn field_list_jumps_to_a_fields_page() {
    use egui_kittest::kittest::Queryable;
    let mut h = form_harness();
    h.state_mut().set_option("panel", "fields").unwrap();
    h.run_steps(4);
    h.get_by_label("fullname");
    h.get_by_label("postcode").click();
    h.run_steps(8);
    assert_eq!(h.state().views[0].current, 1, "postcode is on page 2");
}

#[test]
fn highlight_fields_tints_the_field_area() {
    use egui_kittest::kittest::Queryable;
    let _gpu = gpu();
    let mut h = form_harness();
    h.state_mut().set_option("panel", "none").unwrap();
    for _ in 0..100 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let r = rect(&h, 0).expect("page 1");
    // The field spans x 50..250, y 300..330 (PDF space, y up) on a 300×400 page; sample inside it.
    let at = egui::pos2(r.min.x + r.width() * (60.0 / 300.0), r.min.y + r.height() * (1.0 - 305.0 / 400.0));
    let pixel = |h: &mut Harness<'static, PdfCraftApp>| {
        let img = h.render().expect("renders");
        let ppp = h.ctx.pixels_per_point();
        *img.get_pixel((at.x * ppp) as u32, (at.y * ppp) as u32)
    };
    let before = pixel(&mut h);
    h.get_by_label("Highlight fields").click();
    h.run_steps(4);
    assert!(h.state().views[0].highlight_fields);
    h.get_by_label("Hide field highlights");
    let after = pixel(&mut h);
    assert_ne!(before, after, "the field area is tinted when highlighting is on");
}

#[test]
fn fonts_tab_lists_fonts_and_embedding() {
    use egui_kittest::kittest::Queryable;
    let mut h = form_harness();
    h.state_mut().set_option("dialog", "fonts").unwrap();
    h.run_steps(4);
    h.get_by_label("Helvetica");
    h.get_by_label_contains("Not embedded");
}

#[test]
fn the_hand_tool_pans_by_dragging() {
    let mut h = harness(&[("layout", "continuous"), ("zoom", "150"), ("quick", "hand")]);
    h.run_steps(4);
    let before = rect(&h, 0).expect("page 1 on screen");
    let start = egui::pos2(700.0, 700.0);
    h.hover_at(start);
    h.run_steps(1);
    h.drag_at(start);
    h.run_steps(1);
    for k in 1..=5 {
        h.hover_at(start - egui::vec2(0.0, 60.0 * k as f32));
        h.run_steps(1);
    }
    h.drop_at(start - egui::vec2(0.0, 300.0));
    h.run_steps(4);
    let after = rect(&h, 0).expect("still on screen");
    assert!(before.top() - after.top() > 200.0, "dragging up scrolls down: {before:?} → {after:?}");
    assert!(h.state().views[0].selected_text().is_none(), "no text selection with the hand");
}

#[test]
fn required_fields_get_a_red_border_when_highlighting() {
    let _gpu = gpu();
    let mut h = form_harness();
    h.state_mut().set_option("panel", "none").unwrap();
    h.state_mut().set_option("fields", "on").unwrap();
    let props = pdfcraft_engine::FieldProps { required: Some(true), ..Default::default() };
    h.state_mut().apply_edit(pdfcraft_engine::Edit::SetFieldProps { name: "fullname".into(), props: Box::new(props) });
    for _ in 0..100 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let r = rect(&h, 0).expect("page 1");
    // Just inside the left edge of the field (x 50, y 300..330 on a 300×400 page).
    let ppp = h.ctx.pixels_per_point();
    let at = egui::pos2(r.min.x + r.width() * (50.0 / 300.0) + 0.75, r.min.y + r.height() * (1.0 - 315.0 / 400.0));
    let img = h.render().expect("renders");
    let px = *img.get_pixel((at.x * ppp) as u32, (at.y * ppp) as u32);
    assert!(px[0] > 180 && px[1] < 100 && px[2] < 100, "a red border: {px:?}");
}
