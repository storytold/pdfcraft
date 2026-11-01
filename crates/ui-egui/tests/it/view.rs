//! Page layouts, zoom modes and link navigation, checked by where pages land on screen.

use egui::{Key, Modifiers, MouseWheelUnit, TouchPhase};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
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

use crate::gpu;

fn harness(options: &'static [(&'static str, &'static str)]) -> Harness<'static, PdfCraftApp> {
    harness_stepping(1.0 / 4.0, options)
}

/// [`harness`] with frames `step_dt` seconds apart (a quarter second is kittest's default).
fn harness_stepping(step_dt: f32, options: &'static [(&'static str, &'static str)]) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(step_dt).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
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

fn sidebar_key(h: &mut Harness<'static, PdfCraftApp>, key: Key, modifiers: Modifiers) {
    h.event(egui::Event::ModifiersChanged(modifiers));
    for pressed in [true, false] {
        h.event(egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers });
    }
    h.run_steps(2);
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(1);
}

#[test]
fn pages_sidebar_select_all_keeps_current_page_and_keyboard_scope() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(&[("panel", "pages")]);
    let _ = h.get_by_role_and_label(egui::accesskit::Role::Group, "Pages");
    h.get_by_label("Page 2").click();
    h.run_steps(3);
    assert_eq!(h.state().views[0].current, 1);
    // Moving the pointer is not a keyboard-focus change.
    h.hover_at(rect(&h, 1).unwrap().center());
    h.run_steps(1);
    for _ in 0..2 {
        sidebar_key(&mut h, Key::A, Modifiers::COMMAND);
        assert_eq!(h.state().views[0].target_pages(), vec![0, 1, 2]);
        assert_eq!(h.state().views[0].current, 1, "selecting does not navigate");
    }
    sidebar_key(&mut h, Key::Escape, Modifiers::NONE);
    assert!(h.state().views[0].selected.is_empty());

    // A click on the viewer returns Ctrl+A to text selection, even with Pages open.
    let pos = rect(&h, 1).unwrap().center();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    }
    h.run_steps(3);
    sidebar_key(&mut h, Key::A, Modifiers::COMMAND);
    assert!(h.state().views[0].selected.is_empty(), "viewer shortcut must not select thumbnails");

    // A different document does not inherit the previous sidebar's keyboard ownership.
    h.get_by_label("Page 2").click();
    h.run_steps(2);
    h.state_mut().open_bytes("other.pdf", None, PAGES.to_vec()).unwrap();
    h.run_steps(5);
    sidebar_key(&mut h, Key::A, Modifiers::COMMAND);
    let active = h.state().active.unwrap();
    assert!(h.state().views[active].selected.is_empty());
    h.get_by_label("Page 1").click();
    h.run_steps(2);
    sidebar_key(&mut h, Key::A, Modifiers::COMMAND);
    assert_eq!(h.state().views[active].target_pages(), vec![0, 1, 2]);
    assert!(h.state().views[0].selected.is_empty(), "selection remains document-local");
}

/// Two pages of different sizes: 300×400 then 600×300.
const MIXED: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] >> endobj
4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 600 300] >> endobj
trailer << /Root 1 0 R >>
%%EOF";

fn mixed_harness(options: &'static [(&'static str, &'static str)]) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("mixed.pdf", None, MIXED.to_vec()).expect("opens");
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

/// A vertical wheel event; negative `dy` scrolls down.
fn wheel(unit: MouseWheelUnit, dy: f32, phase: TouchPhase) -> egui::Event {
    egui::Event::MouseWheel { unit, delta: egui::vec2(0.0, dy), phase, modifiers: Modifiers::NONE, source: egui::MouseWheelSource::Unknown }
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
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                assert!(
                    std::time::Instant::now() < deadline,
                    "render readiness timed out: theme={theme}, organize={organize}, errors={:?}",
                    h.state().views[0].page_errors()
                );
                h.run_steps(2);
                if !h.state().render_pending() {
                    // Paint one fresh frame after all requested rasters have arrived.
                    h.run_steps(1);
                    if h.state().render_pending() {
                        continue;
                    }
                    assert!(
                        h.state().views[0].page_errors().is_empty(),
                        "page render errors: theme={theme}, organize={organize}, errors={:?}",
                        h.state().views[0].page_errors()
                    );
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let page = if organize { h.get_by_label("Page 1").rect() } else { rect(&h, 0).unwrap() };
            let at = page.center();
            let ppp = h.ctx.pixels_per_point();
            let before = *h.render().unwrap().get_pixel((at.x * ppp) as u32, (at.y * ppp) as u32);
            assert_eq!(
                before,
                image::Rgba([255, 255, 255, 255]),
                "the synthetic page is white: theme={theme}, organize={organize}, page={page:?}, ppp={ppp}"
            );
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

/// The toggle joins page edges while preserving zoom and leaving the PDF unmodified.
#[test]
fn page_gaps_toggle_joins_edges_without_changing_the_document() {
    let mut h = harness(&[("layout", "continuous"), ("zoom", "50")]);
    let zoom = h.state().views[0].zoom;
    let original = rect(&h, 1).unwrap().top() - rect(&h, 0).unwrap().bottom();
    assert!(original > 1.0);
    h.state_mut().set_option("page-gaps", "off").unwrap();
    h.run_steps(4);
    for page in 0..2 {
        let (a, b) = (rect(&h, page).unwrap(), rect(&h, page + 1).unwrap());
        assert!((b.top() - a.bottom()).abs() < 0.01, "pages must touch: {a:?} {b:?}");
    }
    assert_eq!(h.state().views[0].zoom, zoom);
    let id = h.state().views[0].id;
    assert!(!h.state().session.get(id).unwrap().dirty, "a view option must not edit the PDF");
    assert!(h.state_mut().execute("view.page_gaps"));
    h.run_steps(4);
    assert!(((rect(&h, 1).unwrap().top() - rect(&h, 0).unwrap().bottom()) - original).abs() < 0.01);
}

/// Changing gaps in the organizer must not anchor document pages to the grid viewport.
#[test]
fn page_gaps_in_organizer_do_not_move_the_document_scroll_position() {
    let mut h = harness(&[("layout", "continuous"), ("zoom", "100"), ("page", "2")]);
    let before = rect(&h, 1).unwrap();
    let gap = before.top() - rect(&h, 0).unwrap().bottom();
    assert!(gap > 1.0);
    assert!(h.state_mut().execute("page.organize"));
    h.run_steps(4);
    assert!(h.state().views[0].organize);
    assert!(h.state_mut().execute("view.page_gaps"));
    h.run_steps(2);
    assert!(!h.state().views[0].show_page_gaps);
    assert!(h.state_mut().execute("page.organize"));
    h.run_steps(4);
    let after = rect(&h, 1).unwrap();
    // Keeping the scroll offset moves page 2 up by exactly the removed gap before it.
    assert!((after.top() - before.top() + gap).abs() < 1.0, "stale grid anchoring moved the page: {before:?} -> {after:?}, gap={gap}");
    assert_eq!(h.state().views[0].current, 1);
}

/// A three-page comparison, with the old pane leading while the new document stays active.
fn page_gaps_comparison_harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.set_option("left", "closed").unwrap();
        app.open_bytes("old.pdf", None, PAGES.to_vec()).unwrap();
        app.open_bytes("new.pdf", None, PAGES.to_vec()).unwrap();
        app.set_option("zoom", "50").unwrap();
        app.compare_old = Some(app.views[0].id);
        app.run_compare();
        app
    });
    h.run_steps(4);
    let at = h.state().views[0].viewport_rect().center();
    h.event(egui::Event::PointerMoved(at));
    h.run_steps(2);
    assert_eq!(h.state().compare.as_ref().unwrap().leader, 0);
    assert_eq!(h.state().active, Some(1));
    h
}

/// A page-display menu choice must reach both panes before the old leader synchronizes them.
#[test]
fn page_gaps_comparison_menu_preserves_the_active_panes_choice() {
    use egui_kittest::kittest::Queryable;
    let mut h = page_gaps_comparison_harness();
    h.get_by_label_contains("Page display").click();
    h.run_steps(2);
    h.get_by_label("Show gaps between pages").click();
    h.run_steps(4);
    assert!(h.state().views.iter().all(|v| !v.show_page_gaps), "the old leader must not restore gaps after the active pane's menu action");
}

/// The control setting has the same synchronized behavior and leaves unrelated views alone.
#[test]
fn page_gaps_comparison_control_preserves_both_states() {
    let mut h = page_gaps_comparison_harness();
    for (value, show) in [("off", false), ("on", true)] {
        h.state_mut().set_option("page-gaps", value).unwrap();
        h.run_steps(4);
        assert!(h.state().views.iter().all(|v| v.show_page_gaps == show), "comparison rejected page-gaps={value}");
    }
    h.state_mut().open_bytes("unrelated.pdf", None, PAGES.to_vec()).unwrap();
    h.state_mut().set_option("page-gaps", "off").unwrap();
    assert!(h.state().views[0].show_page_gaps && h.state().views[1].show_page_gaps);
    assert!(!h.state().views[2].show_page_gaps);
}

/// The saved default survives restart and applies to newly opened documents.
#[test]
fn page_gaps_default_survives_restart_and_applies_to_new_documents() {
    let mut app = PdfCraftApp::new();
    app.set_option("default-page-gaps", "off").unwrap();
    let mut restored = PdfCraftApp::new();
    restored.restore(&app.persist());
    restored.open_bytes("pages.pdf", None, PAGES.to_vec()).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| restored);
    h.run_steps(8);
    h.state_mut().set_option("zoom", "50").unwrap();
    h.run_steps(4);
    assert!((rect(&h, 1).unwrap().top() - rect(&h, 0).unwrap().bottom()).abs() < 0.01);
    assert!(h.state_mut().set_option("page-gaps", "invalid").is_err());
}

/// The real menu action removes gutters, shadows and outlines from the seam pixels.
#[test]
fn page_gaps_menu_joins_pages_without_a_shadow_at_the_seam() {
    use egui_kittest::kittest::Queryable;
    let _gpu = gpu();
    let mut h = harness(&[("layout", "continuous"), ("zoom", "50"), ("theme", "light")]);
    h.get_by_label_contains("Page display").click();
    h.run_steps(3);
    h.get_by_label("Show gaps between pages").click();
    h.run_steps(4);
    let (a, b) = (rect(&h, 0).unwrap(), rect(&h, 1).unwrap());
    assert!((b.top() - a.bottom()).abs() < 0.01);
    let ppp = h.ctx.pixels_per_point();
    let image = h.render().expect("renders the gapless view");
    for dy in [-3.0, 0.0, 3.0] {
        let pixel = image.get_pixel((a.center().x * ppp) as u32, ((a.bottom() + dy) * ppp) as u32);
        assert_eq!(*pixel, image::Rgba([255, 255, 255, 255]), "no gap or shadow at the seam");
    }
}

/// Gapless geometry supports mixed sizes and two-page rows and survives document reload.
#[test]
fn page_gaps_work_for_mixed_sizes_and_two_page_rows() {
    let h = mixed_harness(&[("layout", "continuous"), ("zoom", "50"), ("page-gaps", "off")]);
    let (a, b) = (rect(&h, 0).unwrap(), rect(&h, 1).unwrap());
    assert!((b.top() - a.bottom()).abs() < 0.01);
    assert!((a.center().x - b.center().x).abs() < 0.01);
    let mut h = harness(&[("layout", "two-up"), ("zoom", "50"), ("page-gaps", "off")]);
    let (a, b, c) = (rect(&h, 0).unwrap(), rect(&h, 1).unwrap(), rect(&h, 2).unwrap());
    assert!((b.left() - a.right()).abs() < 0.01);
    assert!((c.top() - a.bottom()).abs() < 0.01);
    let info = h.state().session.get(h.state().views[0].id).unwrap().info.clone();
    h.state_mut().views[0].document_replaced(&info);
    assert!(!h.state().views[0].show_page_gaps, "a file reload preserves the view setting");
}

/// Page edges stay adjacent at integer and fractional physical-pixel scales.
#[test]
fn page_gaps_stay_closed_at_fractional_device_scales() {
    for ppp in [1.0, 1.25, 1.5, 2.0] {
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_pixels_per_point(ppp).build_eframe(|_cc| {
            let mut app = PdfCraftApp::new();
            app.open_bytes("pages.pdf", None, PAGES.to_vec()).unwrap();
            for (key, value) in [("left", "closed"), ("panel", "none"), ("layout", "continuous"), ("zoom", "43"), ("page-gaps", "off")] {
                app.set_option(key, value).unwrap();
            }
            app
        });
        h.run_steps(8);
        for page in 0..2 {
            let (a, b) = (rect(&h, page).unwrap(), rect(&h, page + 1).unwrap());
            assert!((b.top() - a.bottom()).abs() < 0.01, "{ppp} pixels/point: {a:?} {b:?}");
        }
    }
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
fn two_page_view_shows_one_spread_at_a_time_until_continuous() {
    use pdfcraft_ui_egui::canvas::PageLayout;
    // Two-page view without continuous scrolling shows just the current spread; the continuous
    // checkbox reveals the rest (#752).
    let mut h = harness(&[("layout", "two-page"), ("zoom", "50")]);
    h.run_steps(4);
    assert_eq!(h.state().views[0].layout, PageLayout::TwoUp);
    assert!(!h.state().views[0].continuous(), "`two-page` is the non-scrolling two-page display");
    assert!(rect(&h, 0).is_some() && rect(&h, 1).is_some(), "the first spread is on screen");
    assert!(rect(&h, 2).is_none(), "the next spread is not shown until paged or scrolled");
    // Turning on continuous scrolling lays every spread out in one column.
    h.state_mut().set_option("continuous", "on").unwrap();
    h.run_steps(4);
    assert!(rect(&h, 2).is_some(), "continuous scrolling reveals the later spreads");
    assert_eq!(h.state().views[0].layout, PageLayout::TwoUp, "the base layout is unchanged");
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
fn single_page_view_prefetches_the_next_page() {
    // Only the shown page is drawn, but the next one renders ahead of the turn: otherwise
    // turning forward shows blank paper until it arrives.
    let mut h = harness(&[("layout", "single"), ("zoom", "50")]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while h.state().render_pending() {
        assert!(std::time::Instant::now() < deadline, "renders settle");
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(rect(&h, 1).is_none(), "page 2 is not shown");
    assert!(h.state().views[0].raster_memory().page_count >= 2, "page 2 is rendered ahead of the turn");
}

#[test]
fn single_page_wheel_turn_draws_the_new_page_at_once() {
    // The frame that turns the page draws the new one. Its jump used to be measured from the
    // old page's place, which scrolled both out of view: a blank frame on every turn forward.
    let mut h = harness_stepping(1.0 / 60.0, &[("layout", "single")]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num0); // fit page
    h.run_steps(4);
    let vp = h.state().views[0].viewport_rect();
    h.hover_at(vp.center());
    h.run_steps(2);
    h.event(wheel(MouseWheelUnit::Line, -1.0, TouchPhase::Move));
    h.step();
    assert_eq!(h.state().views[0].current, 1, "one notch turns one page");
    let r = rect(&h, 1).expect("page 2 drawn in the frame that turns to it");
    assert!(r.min.y >= vp.min.y && r.max.y <= vp.max.y, "page 2 fits on screen: {r:?} in {vp:?}");
}

#[test]
fn single_page_jump_draws_a_fitting_page_where_it_stays() {
    // A page that fits has nothing to scroll, but the jump to it asked for the gap above it:
    // drawn that much too high for a frame, it then hopped into place.
    let mut h = harness_stepping(1.0 / 60.0, &[("layout", "single")]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num0); // fit page
    h.run_steps(4);
    h.state_mut().set_option("page", "2").unwrap();
    h.step();
    let first = rect(&h, 1).expect("page 2 drawn in the frame after the jump");
    h.run_steps(4);
    let settled = rect(&h, 1).expect("page 2");
    assert!((first.min.y - settled.min.y).abs() < 0.5, "page 2 drawn where it settles: {first:?} then {settled:?}");
}

#[test]
fn single_page_wheel_pans_instead_when_zoomed_in() {
    // The zoomed-in guard: with room to pan, the wheel pans and must not turn pages.
    let mut h = harness(&[("layout", "single"), ("zoom", "200")]);
    h.run_steps(4);
    let at = h.state().views[0].viewport_rect().center();
    h.hover_at(at);
    h.run_steps(1);
    h.event(wheel(MouseWheelUnit::Point, -60.0, TouchPhase::Move));
    h.run_steps(4);
    assert_eq!(h.state().views[0].current, 0, "zoomed-in wheel pans, it does not turn the page");
}

#[test]
fn single_page_wheel_at_the_bottom_edge_turns_to_the_next_page_top_and_back() {
    // #1924: zoomed in so the page is taller than the window, the wheel pans; at the bottom
    // edge one more notch shows the next page at its top, and at the top edge one notch up
    // shows the previous page at its bottom.
    let mut h = harness_stepping(1.0 / 60.0, &[("layout", "single"), ("zoom", "200")]);
    let at = h.state().views[0].viewport_rect().center();
    h.hover_at(at);
    h.run_steps(4);
    // Notches pan page 1 down; the page turns only after the view stopped at the bottom.
    let (mut notches, mut stalled) = (0, 0);
    let mut last = rect(&h, 0).expect("page 1 on screen").min.y;
    while h.state().views[0].current == 0 && notches < 80 {
        h.event(wheel(MouseWheelUnit::Line, -1.0, TouchPhase::Move));
        h.run_steps(12);
        notches += 1;
        if let Some(r) = rect(&h, 0) {
            stalled = if (r.min.y - last).abs() < 0.01 { stalled + 1 } else { 0 };
            last = r.min.y;
        }
    }
    assert!(notches > 2, "the page panned before it turned: {notches} notches");
    assert_eq!(h.state().views[0].current, 1, "a notch at the bottom edge shows the next page");
    assert!(stalled <= 1, "no notch was wasted at the edge");
    let top = rect(&h, 1).expect("page 2 on screen").min.y;
    assert!(top > h.state().views[0].viewport_rect().min.y, "page 2 shows its top edge: {top}");
    h.event(wheel(MouseWheelUnit::Line, 1.0, TouchPhase::Move));
    h.run_steps(30);
    assert_eq!(h.state().views[0].current, 0, "a notch at the top edge shows the previous page");
    let r = rect(&h, 0).expect("page 1 on screen");
    assert!(r.max.y < h.state().views[0].viewport_rect().max.y + 40.0, "page 1 shows its bottom edge: {r:?}");
}

#[test]
fn single_page_swipe_that_reaches_the_edge_does_not_turn_but_a_fresh_one_does() {
    // Momentum that carries a swipe to the bottom edge must not go on to turn the page.
    let mut h = harness_stepping(1.0 / 60.0, &[("layout", "single"), ("zoom", "200")]);
    let at = h.state().views[0].viewport_rect().center();
    h.hover_at(at);
    h.run_steps(4);
    let send = |h: &mut Harness<'static, PdfCraftApp>, phase, dy| {
        h.event(wheel(MouseWheelUnit::Point, dy, phase));
        h.step();
    };
    // One long swipe: far more motion than the page has, in one gesture.
    send(&mut h, TouchPhase::Start, 0.0);
    for _ in 0..120 {
        send(&mut h, TouchPhase::Move, -40.0);
    }
    send(&mut h, TouchPhase::End, 0.0);
    h.run_steps(30);
    assert_eq!(h.state().views[0].current, 0, "the swipe that panned to the edge does not turn");
    // A new swipe at the edge turns exactly one page.
    send(&mut h, TouchPhase::Start, 0.0);
    for _ in 0..6 {
        send(&mut h, TouchPhase::Move, -25.0);
    }
    send(&mut h, TouchPhase::End, 0.0);
    h.run_steps(30);
    assert_eq!(h.state().views[0].current, 1, "a fresh swipe at the edge turns one page");
    let top = rect(&h, 1).expect("page 2 on screen").min.y;
    assert!(
        top >= h.state().views[0].viewport_rect().min.y,
        "page 2 starts at its top after the swipe: {top} in {:?}",
        h.state().views[0].viewport_rect()
    );
}

#[test]
fn stepping_past_either_end_leaves_the_view_alone() {
    // Paging on past the first or last page must not jump the page back to its top.
    for (page, forward) in [(2, true), (0, false)] {
        let mut h = harness_stepping(1.0 / 60.0, &[("layout", "single"), ("zoom", "200")]);
        h.state_mut().views[0].go_to_page(page);
        h.run_steps(20);
        let at = h.state().views[0].viewport_rect().center();
        h.hover_at(at);
        h.run_steps(4);
        // Pan the page off its edge so a jump would show.
        h.event(wheel(MouseWheelUnit::Line, -1.0, TouchPhase::Move));
        h.run_steps(30);
        let before = rect(&h, page).expect("page on screen");
        h.state_mut().views[0].step_page(forward);
        h.run_steps(30);
        assert_eq!(h.state().views[0].current, page);
        assert_eq!(rect(&h, page), Some(before), "stepping past the end (forward={forward}) moved the page");
    }
}

#[test]
fn wheel_turns_one_page_per_notch_and_per_trackpad_swipe() {
    // At a real frame rate: brisk notches each turn a page, and one trackpad swipe turns one
    // page, its momentum included (macOS sends the momentum as a second Start…End).
    let mut h = harness_stepping(1.0 / 60.0, &[("layout", "single"), ("zoom", "50")]);
    let at = h.state().views[0].viewport_rect().center();
    h.hover_at(at);
    h.run_steps(2);
    for _ in 0..2 {
        h.event(wheel(MouseWheelUnit::Line, -1.0, TouchPhase::Move));
        h.run_steps(5);
    }
    h.run_steps(30);
    assert_eq!(h.state().views[0].current, 2, "two notches 80 ms apart turn two pages");
    let mut send = |phase, dy| {
        h.event(wheel(MouseWheelUnit::Point, dy, phase));
        h.step();
    };
    send(TouchPhase::Start, 0.0);
    for _ in 0..6 {
        send(TouchPhase::Move, 25.0);
    }
    send(TouchPhase::End, 0.0);
    send(TouchPhase::Start, 0.0);
    let mut d = 25.0;
    while d > 0.5 {
        send(TouchPhase::Move, d);
        d *= 0.92;
    }
    send(TouchPhase::End, 0.0);
    h.run_steps(30);
    assert_eq!(h.state().views[0].current, 1, "one swipe back turns one page back");
}

#[test]
fn precise_touchpad_wheel_scrolls_one_to_one() {
    // A precision touchpad's pixel delta scrolls the page by that amount in the same frame,
    // instead of egui easing it over several frames so the page lags the finger (#759). A single
    // 200 px precise event is well over egui's 8 px smoothing threshold, so without the fix only
    // a fraction would land on the first frame.
    let mut h = harness_stepping(1.0 / 60.0, &[("layout", "continuous"), ("zoom", "100")]);
    let at = h.state().views[0].viewport_rect().center();
    h.hover_at(at);
    h.run_steps(2);
    let before = rect(&h, 0).expect("page 1 on screen").min.y;
    h.event(wheel(MouseWheelUnit::Point, -200.0, TouchPhase::Move));
    // A few frames for the event to reach the ScrollArea and the new offset to be drawn. The
    // page jumps the full 200 px at once; an eased path would ramp there over ~0.1 s (6 frames).
    h.run_steps(4);
    let moved = before - rect(&h, 0).expect("page 1 still laid out").min.y;
    assert!(moved > 190.0, "a 200 px precise swipe scrolls ~200 px at once, not an eased fraction: moved {moved}");
    // And it stops with the finger: idle frames must not drift on a leftover easing tail.
    let settled = rect(&h, 0).unwrap().min.y;
    h.run_steps(20);
    let drift = settled - rect(&h, 0).unwrap().min.y;
    assert!(drift.abs() < 2.0, "no easing tail after the swipe: drifted {drift}");
}

#[test]
fn rapid_wheel_notches_accelerate_and_follow_touchpad_input() {
    use pdfcraft_ui_egui::scroll_speed::ScrollSpeed;

    let mut h = harness_stepping(1.0 / 60.0, &[("layout", "continuous"), ("zoom", "300")]);
    h.state_mut().set_option("animation_effects", "off").unwrap();
    let at = h.state().views[0].viewport_rect().center();
    h.hover_at(at);
    h.run_steps(2);

    h.event(wheel(MouseWheelUnit::Line, -1.0, TouchPhase::Move));
    let single = scroll_over(&mut h, 2);
    let base = ScrollSpeed::default().line_points();
    assert!((single - base).abs() < 1.0, "one notch keeps the configured base: moved {single}, base {base}");

    // Let the isolated notch expire, then spin five notches one frame apart. The burst must move
    // farther than five base steps without adding work on idle frames.
    h.run_steps(30);
    let mut burst = 0.0;
    for _ in 0..5 {
        h.event(wheel(MouseWheelUnit::Line, -1.0, TouchPhase::Move));
        burst += scroll_over(&mut h, 2);
    }
    assert!(burst > 7.0 * base, "a rapid five-notch spin accelerates: moved {burst}, base {base}");

    // A line notch inside the precise path's suppression window keeps its base share (#1072):
    // suppressing egui's touchpad tail must not swallow mixed-device input.
    h.run_steps(30);
    h.event(wheel(MouseWheelUnit::Point, -20.0, TouchPhase::Move));
    scroll_over(&mut h, 2);
    // Arrive near the old suppression deadline: the line's new easing tail must extend it.
    h.run_steps(11);
    h.event(wheel(MouseWheelUnit::Line, -1.0, TouchPhase::Move));
    let after_touchpad = scroll_over(&mut h, 2);
    assert!(after_touchpad >= base, "a notch after point input is not dropped: moved {after_touchpad}, base {base}");
    let delayed = scroll_over(&mut h, 20);
    assert!(delayed.abs() < 2.0, "the reconstructed notch has no delayed easing copy: moved {delayed}");

    h.event(wheel(MouseWheelUnit::Point, -20.0, TouchPhase::Move));
    scroll_over(&mut h, 2);
    h.event(wheel(MouseWheelUnit::Page, -1.0, TouchPhase::Move));
    let page_after_touchpad = scroll_over(&mut h, 2);
    assert!(page_after_touchpad > base, "a page event after point input is preserved: moved {page_after_touchpad}");
    let delayed_page = scroll_over(&mut h, 20);
    assert!(delayed_page.abs() < 2.0, "the reconstructed page has no delayed easing copy: moved {delayed_page}");
}

#[test]
fn changing_continuous_layout_resets_wheel_acceleration() {
    use pdfcraft_ui_egui::scroll_speed::ScrollSpeed;

    let mut h = harness_stepping(1.0 / 60.0, &[("layout", "continuous"), ("zoom", "300")]);
    h.state_mut().set_option("animation_effects", "off").unwrap();
    let at = h.state().views[0].viewport_rect().center();
    h.hover_at(at);
    h.run_steps(2);
    for _ in 0..5 {
        h.event(wheel(MouseWheelUnit::Line, -1.0, TouchPhase::Move));
        scroll_over(&mut h, 2);
    }

    h.state_mut().views[0].set_continuous(false);
    h.run_steps(3);
    h.event(wheel(MouseWheelUnit::Line, -1.0, TouchPhase::Move));
    let moved = scroll_over(&mut h, 2);
    let base = ScrollSpeed::default().line_points();
    assert!((moved - base).abs() < 1.0, "a layout jump clears the old burst: moved {moved}, base {base}");
}

/// Steps `frames` frames and returns how far the document scrolled down over them, from the
/// pages on screen in each pair of consecutive frames (a page can leave the screen on the way).
/// Used by wheel and non-macOS glide tests below.
fn scroll_over(h: &mut Harness<'static, PdfCraftApp>, frames: usize) -> f32 {
    let tops = |h: &Harness<'static, PdfCraftApp>| (0..3).map(|p| rect(h, p).map(|r| r.min.y)).collect::<Vec<_>>();
    let mut moved = 0.0;
    for _ in 0..frames {
        let before = tops(h);
        h.step();
        let after = tops(h);
        let step = before.iter().zip(&after).find_map(|(b, a)| Some(b.as_ref()? - a.as_ref()?));
        moved += step.expect("a page stays on screen from one frame to the next");
    }
    moved
}

/// A two-finger swipe down the page, 25 points a frame for six frames, lifted the frame after
/// (the axis stop Wayland sends), at 300 % so the document is long enough to glide in: how far
/// it scrolled by the frame after the lift (the page is drawn a frame behind its scroll).
/// Same gate as its callers below.
#[cfg(not(target_os = "macos"))]
fn swipe_and_lift(h: &mut Harness<'static, PdfCraftApp>) -> f32 {
    let at = h.state().views[0].viewport_rect().center();
    h.hover_at(at);
    h.run_steps(2);
    h.event(wheel(MouseWheelUnit::Point, -25.0, TouchPhase::Start));
    let mut moved = scroll_over(h, 1);
    for _ in 0..6 {
        h.event(wheel(MouseWheelUnit::Point, -25.0, TouchPhase::Move));
        moved += scroll_over(h, 1);
    }
    h.event(wheel(MouseWheelUnit::Point, 0.0, TouchPhase::End));
    moved + scroll_over(h, 2)
}

/// The touchpad glide is Wayland's: macOS sends its own momentum and the web none (#531).
#[cfg(not(target_os = "macos"))]
#[test]
fn touchpad_swipe_glides_on_after_the_fingers_lift_and_stops() {
    let mut h = harness_stepping(1.0 / 60.0, &[("layout", "continuous"), ("zoom", "300")]);
    let swiped = swipe_and_lift(&mut h);
    // The seven 25-point motions, 1:1 (#759).
    assert!(swiped >= 174.0, "the swipe scrolls 1:1: moved {swiped}");
    // 1500 points a second at the lift: about 190 points over the next 10 frames.
    let glided = scroll_over(&mut h, 10);
    assert!(glided > 100.0, "the page glides on after the lift: moved {glided} more");
    let rest = scroll_over(&mut h, 180);
    let drift = scroll_over(&mut h, 20);
    assert!(drift.abs() < 0.5, "the glide slows to a stop: still drifting {drift}");
    let total = swiped - 175.0 + glided + rest;
    assert!(total > 400.0 && total < 600.0, "about the lift speed × 0.35 s: glided {total}");
}

#[cfg(not(target_os = "macos"))]
#[test]
fn touching_the_touchpad_again_or_turning_animations_off_stops_the_glide() {
    let mut h = harness_stepping(1.0 / 60.0, &[("layout", "continuous"), ("zoom", "300")]);
    swipe_and_lift(&mut h);
    assert!(scroll_over(&mut h, 3) > 30.0, "gliding");
    h.event(wheel(MouseWheelUnit::Point, 0.0, TouchPhase::Start));
    scroll_over(&mut h, 2);
    let drift = scroll_over(&mut h, 20);
    assert!(drift.abs() < 0.5, "fingers back down catch the page: drifted {drift}");

    // A click (a pressed touchpad, or a tap with tap-to-click) catches it too.
    let mut h = harness_stepping(1.0 / 60.0, &[("layout", "continuous"), ("zoom", "300")]);
    swipe_and_lift(&mut h);
    assert!(scroll_over(&mut h, 3) > 30.0, "gliding");
    let at = h.state().views[0].viewport_rect().center();
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    scroll_over(&mut h, 2);
    let drift = scroll_over(&mut h, 20);
    assert!(drift.abs() < 0.5, "a click catches the page: drifted {drift}");

    // Animation effects off: the page stops with the fingers, as before #531.
    let mut h = harness_stepping(1.0 / 60.0, &[("layout", "continuous"), ("zoom", "300")]);
    h.state_mut().set_option("animation_effects", "off").unwrap();
    let swiped = swipe_and_lift(&mut h);
    assert!((swiped - 175.0).abs() < 1.0, "the swipe still scrolls 1:1: moved {swiped}");
    let drift = scroll_over(&mut h, 20);
    assert!(drift.abs() < 0.5, "no glide with animations off: drifted {drift}");
}

/// The 40 frames from one wheel notch on, with the system asking for no animations (Windows'
/// Animation effects turned off) or not, and Preferences ▸ Animation effects as given: how far
/// page 1 has moved up in each frame, or with ⌘/Ctrl held, the view's zoom.
fn frames_after_a_notch(system_reduced: bool, animation_effects: &str, zooming: bool) -> Vec<f32> {
    let mut h = harness_stepping(1.0 / 60.0, &[("layout", "continuous"), ("zoom", "100")]);
    h.state_mut().motion.reduced = system_reduced;
    h.state_mut().set_option("animation_effects", animation_effects).unwrap();
    let at = h.state().views[0].viewport_rect().center();
    h.hover_at(at);
    h.run_steps(2);
    let before = rect(&h, 0).expect("page 1 on screen").min.y;
    let (delta, modifiers) = if zooming { (egui::vec2(0.0, 1.0), Modifiers::COMMAND) } else { (egui::vec2(0.0, -1.0), Modifiers::NONE) };
    h.event(egui::Event::MouseWheel {
        unit: MouseWheelUnit::Line,
        delta,
        phase: TouchPhase::Move,
        modifiers,
        source: egui::MouseWheelSource::Unknown,
    });
    (0..40)
        .map(|_| {
            h.step();
            if zooming { h.state().views[0].zoom } else { before - rect(&h, 0).expect("page 1 still laid out").min.y }
        })
        .collect()
}

/// The system's answer and the Animation effects choice, for each way to have them off and
/// each way to have them on.
const AT_ONCE: [(bool, &str); 3] = [(true, "system"), (false, "off"), (true, "off")];
const EASED: [(bool, &str); 3] = [(false, "system"), (false, "on"), (true, "on")];

#[test]
fn a_wheel_notch_scrolls_at_once_with_animation_effects_off() {
    // Eased, egui spreads a notch over about a quarter of a second. At once, the page moves the
    // notch's whole distance in one frame.
    let notch = pdfcraft_ui_egui::scroll_speed::ScrollSpeed::default().line_points();
    for (system_reduced, effects) in AT_ONCE {
        let moved = frames_after_a_notch(system_reduced, effects, false);
        // The frame the notch arrives in scrolls; the next one draws the page where it stays.
        assert!(
            moved[2..].iter().all(|y| (y - notch).abs() < 0.5),
            "{effects}, system reduced {system_reduced}: one notch is {notch} points, all at once: {moved:?}"
        );
    }
    for (system_reduced, effects) in EASED {
        let moved = frames_after_a_notch(system_reduced, effects, false);
        assert!(moved[2] < notch * 0.75, "{effects}, system reduced {system_reduced}: egui eases the notch: {moved:?}");
        assert!(
            (moved[39] - notch).abs() < 1.0,
            "{effects}, system reduced {system_reduced}: easing changes how long a notch takes, not how far it goes: {moved:?}"
        );
    }
}

#[test]
fn the_wheel_zooms_a_whole_step_at_once_with_animation_effects_off() {
    // ⌘/Ctrl with the wheel: egui eases the zoom as it eases scrolling.
    let step = frames_after_a_notch(true, "system", true)[0];
    assert!(step > 1.1, "the notch zoomed in: {step}");
    for (system_reduced, effects) in AT_ONCE {
        let zooms = frames_after_a_notch(system_reduced, effects, true);
        assert!(
            zooms.iter().all(|zoom| *zoom == step),
            "{effects}, system reduced {system_reduced}: the whole step in the frame the notch arrives, and no more after it: {zooms:?}"
        );
    }
    for (system_reduced, effects) in EASED {
        let zooms = frames_after_a_notch(system_reduced, effects, true);
        assert!(
            zooms[0] > 1.0 && zooms[0] < 1.0 + (step - 1.0) * 0.75,
            "{effects}, system reduced {system_reduced}: egui eases the notch: {zooms:?}"
        );
        assert!(
            (zooms[39] / step - 1.0).abs() < 0.01,
            "{effects}, system reduced {system_reduced}: easing changes how long a zoom step takes, not how far it goes: {zooms:?}"
        );
    }
}

#[test]
fn page_display_commands_switch_layouts() {
    let mut h = harness(&[]);
    h.state_mut().execute("view.layout.single");
    assert_eq!(h.state().views[0].layout, pdfcraft_ui_egui::canvas::PageLayout::Single);
    // The cover toggle refuses outside two-page view instead of arming a hidden flag.
    assert!(!h.state().views[0].cover);
    assert!(!h.state_mut().execute("view.layout.cover"));
    assert!(!h.state().views[0].cover);
    h.state_mut().execute("view.layout.two_up");
    assert_eq!(h.state().views[0].layout, pdfcraft_ui_egui::canvas::PageLayout::TwoUp);
    assert!(h.state_mut().execute("view.layout.cover"));
    assert!(h.state().views[0].cover, "the cover toggle flips");
    assert!(h.state_mut().execute("view.layout.cover"));
    assert!(!h.state().views[0].cover, "toggling twice restores");
    // Continuous scrolling is an independent toggle over the base layout (#752).
    assert!(h.state().views[0].continuous(), "the default view scrolls continuously");
    h.state_mut().execute("view.continuous");
    h.run_steps(3);
    assert!(!h.state().views[0].continuous(), "the checkbox turns continuous scrolling off");
    assert_eq!(h.state().views[0].layout, pdfcraft_ui_egui::canvas::PageLayout::TwoUp, "toggling scrolling leaves the base layout");
    h.state_mut().execute("view.layout.single");
    assert_eq!(h.state().views[0].layout, pdfcraft_ui_egui::canvas::PageLayout::Single);
    assert!(!h.state().views[0].continuous(), "switching the base layout leaves scrolling off");
}

#[test]
fn layout_option_rejects_typos() {
    let mut h = harness(&[]);
    assert!(h.state_mut().set_option("layout", "singel").is_err());
    assert!(h.state_mut().set_option("default-layout", "bogus").is_err());
    assert!(h.state_mut().set_option("cover", "maybe").is_err());
    assert_eq!(h.state().views[0].layout, pdfcraft_ui_egui::canvas::PageLayout::Single);
    assert!(h.state().views[0].continuous(), "a rejected typo leaves the default display unchanged");
    assert!(h.state_mut().set_option("cover", "on").is_err(), "cover needs two-page view first");
    h.state_mut().set_option("layout", "two-up").unwrap();
    h.state_mut().set_option("cover", "on").unwrap();
    assert!(h.state().views[0].cover);
    h.state_mut().set_option("cover", "off").unwrap();
    assert!(!h.state().views[0].cover);
}

#[test]
fn view_options_without_a_document() {
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    assert!(app.set_option("default-layout", "single").is_ok(), "the default needs no document");
    assert_eq!(app.view_defaults.layout, pdfcraft_ui_egui::canvas::PageLayout::Single);
    assert!(app.set_option("cover", "on").is_err(), "cover needs an open document");
    assert!(app.set_option("layout", "single").is_err(), "layout needs an open document");
}

#[test]
fn default_page_display_is_used_for_new_documents() {
    use pdfcraft_ui_egui::canvas::PageLayout;
    // The factory default is single page, scrolling continuously, without snap jumps.
    assert_eq!(PdfCraftApp::new().view_defaults.layout, PageLayout::Single);
    assert!(PdfCraftApp::new().view_defaults.continuous);
    let mut h = harness(&[]);
    h.state_mut().set_option("default-layout", "single").unwrap();
    h.state_mut().open_bytes("other.pdf", None, PAGES.to_vec()).expect("opens");
    assert_eq!(h.state().views[1].layout, PageLayout::Single);
    // So do documents the app creates itself (blank, combined, extracted, from images…).
    assert!(h.state_mut().execute("create.blank"));
    assert_eq!(h.state().views[2].layout, PageLayout::Single);
    // Persisted preferences survive a restart.
    let saved = h.state().persist();
    let mut fresh = PdfCraftApp::new();
    fresh.set_option("language", "en").unwrap();
    fresh.restore(&saved);
    assert_eq!(fresh.view_defaults.layout, PageLayout::Single);
    // Garbage keeps the previous default; casing is forgiven; legacy files lack the key.
    fresh.restore(r#"{"default_layout":"bogus"}"#);
    assert_eq!(fresh.view_defaults.layout, PageLayout::Single);
    fresh.restore(r#"{"default_layout":"TWO-UP"}"#);
    assert_eq!(fresh.view_defaults.layout, PageLayout::TwoUp);
    let mut legacy = PdfCraftApp::new();
    legacy.set_option("language", "en").unwrap();
    legacy.restore("{}");
    assert_eq!(legacy.view_defaults.layout, PageLayout::Single);
    assert!(legacy.view_defaults.continuous, "a legacy file without the key keeps the factory default");
}

#[test]
fn default_zoom_is_used_for_new_documents() {
    use pdfcraft_ui_egui::canvas::Fit;
    let fit_zoom = |app: &PdfCraftApp, i: usize| (app.views[i].fit, app.views[i].zoom);
    let mut h = harness(&[]);
    // The factory default fits the width.
    assert_eq!(h.state().views[0].fit, Fit::Width);
    h.state_mut().set_option("default-zoom", "fit-page").unwrap();
    assert!(h.state_mut().execute("create.blank"));
    assert_eq!(h.state().views[1].fit, Fit::Page);
    h.state_mut().set_option("default-zoom", "150%").unwrap();
    h.state_mut().open_bytes("other.pdf", None, PAGES.to_vec()).expect("opens");
    assert_eq!(fit_zoom(h.state(), 2), (Fit::None, 1.5));
    // A PDF that asks for a zoom gets it.
    let asks = String::from_utf8_lossy(PAGES).replace("/Pages 2 0 R >>", "/Pages 2 0 R /OpenAction [3 0 R /Fit] >>");
    h.state_mut().open_bytes("asks.pdf", None, asks.into_bytes()).expect("opens");
    assert_eq!(h.state().views[3].fit, Fit::Page);
    // Typos and zooms the view can't show are refused.
    for bad in ["huge", "fit-height", "5", "9000%"] {
        assert!(h.state_mut().set_option("default-zoom", bad).is_err(), "{bad}");
    }
    // Saved settings keep it, and garbage leaves it alone.
    let saved = h.state().persist();
    let mut fresh = PdfCraftApp::new();
    fresh.set_option("language", "en").unwrap();
    fresh.restore(&saved);
    fresh.restore(r#"{"default_zoom":"bogus"}"#);
    fresh.open_bytes("again.pdf", None, PAGES.to_vec()).expect("opens");
    assert_eq!(fit_zoom(&fresh, 0), (Fit::None, 1.5));
}

#[test]
fn the_rail_zoom_field_sets_a_typed_zoom() {
    use egui::accesskit::Role;
    use egui_kittest::kittest::Queryable;
    use pdfcraft_ui_egui::canvas::Fit;
    let mut h = harness(&[]);
    let type_zoom = |h: &mut Harness<'static, PdfCraftApp>, typed: &str, key: Key| {
        h.get_by_role_and_label(Role::TextInput, "Zoom").click();
        h.run_steps(2);
        h.get_by_role_and_label(Role::TextInput, "Zoom").type_text(typed);
        h.run_steps(1);
        h.key_press(key);
        h.run_steps(3);
    };
    type_zoom(&mut h, "150", Key::Enter);
    assert_eq!((h.state().views[0].fit, h.state().views[0].zoom), (Fit::None, 1.5));
    type_zoom(&mut h, "62.5 %", Key::Enter);
    assert_eq!(h.state().views[0].zoom, 0.625);
    for (typed, key) in [("abc", Key::Enter), ("0", Key::Enter), ("300", Key::Escape)] {
        type_zoom(&mut h, typed, key);
        assert_eq!(h.state().views[0].zoom, 0.625, "{typed}");
    }
    type_zoom(&mut h, "9000", Key::Enter);
    assert_eq!(h.state().views[0].zoom, 64.0);
    h.state_mut().views[0].set_zoom(2.0);
    h.run_steps(2);
    assert_eq!(h.get_by_role_and_label(Role::TextInput, "Zoom").value().as_deref(), Some("200%"));
}

#[test]
fn always_use_my_default_zoom_and_page_layout_ignores_what_the_pdf_asks_for() {
    use pdfcraft_ui_egui::canvas::{Fit, PageLayout};
    // Opens on page 2 at fit page, two pages side by side.
    let asks = String::from_utf8_lossy(PAGES).replace("/Pages 2 0 R >>", "/Pages 2 0 R /OpenAction [4 0 R /Fit] /PageLayout /TwoPageLeft >>");
    let open = |always: Option<&str>| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        if let Some(on) = always {
            app.set_option("always-default-view", on).unwrap();
        }
        app.open_bytes("asks.pdf", None, asks.clone().into_bytes()).expect("opens");
        let v = &app.views[0];
        (v.fit, v.layout, v.continuous, v.current)
    };
    // Off by default: the document's view wins.
    assert_eq!(open(None), (Fit::Page, PageLayout::TwoUp, false, 1));
    assert_eq!(open(Some("off")), (Fit::Page, PageLayout::TwoUp, false, 1));
    // On: Default zoom and Default page display; the page it opens at is still the document's.
    assert_eq!(open(Some("on")), (Fit::Width, PageLayout::Single, true, 1));
    assert!(PdfCraftApp::new().set_option("always-default-view", "yes").is_err());
}

#[test]
fn always_use_my_default_zoom_and_page_layout_survives_a_restart() {
    let mut app = PdfCraftApp::new();
    assert!(!app.always_default_view, "off by default");
    app.always_default_view = true;
    let mut restarted = PdfCraftApp::new();
    restarted.restore(&app.persist());
    assert!(restarted.always_default_view, "the choice is kept");
    restarted.restore(r#"{"always_default_view":"bogus"}"#);
    assert!(restarted.always_default_view, "garbage leaves it alone");
    let mut legacy = PdfCraftApp::new();
    legacy.restore("{}");
    assert!(!legacy.always_default_view, "a legacy file without the key keeps it off");
}

#[test]
fn preferences_turn_on_always_use_my_default_zoom_and_page_layout() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(&[]);
    assert!(h.state_mut().execute("app.preferences"));
    h.run_steps(3);
    h.get_by_label("Always use my default zoom and page layout").scroll_to_me();
    h.run_steps(4);
    h.get_by_label("Always use my default zoom and page layout").click();
    h.run_steps(2);
    assert!(h.state().always_default_view);
}

#[test]
fn preferences_set_the_default_zoom() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(&[]);
    assert!(h.state_mut().execute("app.preferences"));
    h.run_steps(3);
    h.get_by_label("Zoom to page level").scroll_to_me();
    h.run_steps(4);
    h.get_by_label("Zoom to page level").click();
    h.run_steps(2);
    assert!(h.state_mut().execute("create.blank"));
    assert_eq!(h.state().views[1].fit, pdfcraft_ui_egui::canvas::Fit::Page);
}

#[test]
fn rail_page_display_menu_offers_acrobats_view_choices() {
    use egui_kittest::kittest::Queryable;
    use pdfcraft_ui_egui::canvas::{Fit, PageLayout};
    let mut h = harness(&[]);
    h.run_steps(4);
    let pick = |h: &mut Harness<'static, PdfCraftApp>, item: &str| {
        h.get_by_label_contains("Page display:").click();
        h.run_steps(3);
        h.get_by_label(item).click();
        h.run_steps(4);
    };
    let shown = |h: &Harness<'static, PdfCraftApp>| (h.state().views[0].layout, h.state().views[0].continuous(), h.state().views[0].fit);
    pick(&mut h, "Fit one full page");
    assert_eq!(shown(&h), (PageLayout::Single, false, Fit::Page), "one full page at a time, not scrolling");
    // The whole page fits at once, so the wheel turns pages without zooming out first.
    let at = h.state().views[0].viewport_rect().center();
    h.hover_at(at);
    h.run_steps(1);
    h.event(wheel(MouseWheelUnit::Line, -1.0, TouchPhase::Move));
    h.run_steps(4);
    assert_eq!(h.state().views[0].current, 1);
    pick(&mut h, "Actual size");
    assert_eq!((h.state().views[0].fit, h.state().views[0].zoom), (Fit::None, 1.0));
    pick(&mut h, "Zoom to page level");
    assert_eq!(h.state().views[0].fit, Fit::Page);
    pick(&mut h, "Fit to width scrolling");
    assert_eq!(shown(&h), (PageLayout::Single, true, Fit::Width), "single page, scrolling continuously");
}

#[test]
fn rail_page_display_button_switches_layouts() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(&[]);
    h.run_steps(4);
    h.get_by_label_contains("Page display").click();
    h.run_steps(3);
    h.get_by_label("Single page").click();
    h.run_steps(4);
    assert_eq!(h.state().views[0].layout, pdfcraft_ui_egui::canvas::PageLayout::Single);
}

#[test]
fn view_menu_page_display_uses_the_same_commands() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(&[]);
    h.run_steps(4);
    crate::support::open_menu(&mut h, "View");
    crate::support::menu_row(&h, "Page display").hover();
    h.run_steps(3);
    h.get_by_label("Single page").click();
    h.run_steps(4);
    assert_eq!(h.state().views[0].layout, pdfcraft_ui_egui::canvas::PageLayout::Single);
}

#[test]
fn palette_runs_page_display_commands_from_the_keyboard() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(&[]);
    h.state_mut().set_option("palette", "single page").unwrap();
    h.run_steps(3);
    h.get_by_label("Single page");
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert!(!h.state().palette_open, "Enter runs the top hit and closes the palette");
    assert_eq!(h.state().views[0].layout, pdfcraft_ui_egui::canvas::PageLayout::Single);
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
fn fit_page_holds_zoom_across_mixed_page_sizes() {
    // Scrolling views fit their largest page, so scrolling past other sizes must not jump
    // the zoom (#214).
    let mut h = mixed_harness(&[("layout", "continuous")]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num0); // fit page
    h.run_steps(4);
    let z1 = h.state().views[0].zoom;
    h.state_mut().set_option("page", "2").unwrap();
    h.run_steps(4);
    assert_eq!(h.state().views[0].current, 1);
    assert_eq!(h.state().views[0].zoom, z1, "another size scrolls past at the same zoom");
}

#[test]
fn single_page_fit_page_fits_the_shown_page() {
    // Single-page view fits the page it shows: each page fills the viewport on its own.
    let mut h = mixed_harness(&[("layout", "single")]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num0); // fit page
    h.run_steps(4);
    let vp = h.state().views[0].viewport_rect();
    let r1 = rect(&h, 0).expect("page 1");
    assert!(r1.width() <= vp.width() && r1.height() <= vp.height(), "page 1 fits: {r1:?} in {vp:?}");
    let z1 = h.state().views[0].zoom;
    h.state_mut().set_option("page", "2").unwrap();
    h.run_steps(4);
    let r2 = rect(&h, 1).expect("page 2");
    assert!(r2.width() <= vp.width() && r2.height() <= vp.height(), "page 2 fits: {r2:?} in {vp:?}");
    assert_ne!(h.state().views[0].zoom, z1, "the wider page refits instead of keeping page 1's zoom");
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

/// Bookmarks and a link with positioned destinations (ISO 32000-2 §12.3.2.2). Page 2's crop box
/// is away from the origin (x 50..350, y 100..500), so its user-space y 420 is 20% down the page.
const DESTS: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /Outlines 10 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /Annots [20 0 R] >> endobj
4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 400 600] /CropBox [50 100 350 500] >> endobj
5 0 obj << /Type /Page /Parent 2 0 R >> endobj
10 0 obj << /Type /Outlines /First 11 0 R /Last 13 0 R /Count 3 >> endobj
11 0 obj << /Title (Section 1.1 Shapes) /Parent 10 0 R /Next 12 0 R /Dest [4 0 R /XYZ 0 420 0] >> endobj
12 0 obj << /Title (Zoomed in) /Parent 10 0 R /Next 13 0 R /Dest [4 0 R /XYZ 125 300 3] >> endobj
13 0 obj << /Title (Broken) /Parent 10 0 R /Dest [5 0 R /XYZ (left) /top true] >> endobj
20 0 obj << /Type /Annot /Subtype /Link /Rect [50 300 250 350] /Border [0 0 0] /Dest [4 0 R /FitH 420] >> endobj
trailer << /Root 1 0 R >>
%%EOF";

fn dests_harness() -> Harness<'static, PdfCraftApp> {
    bookmarks_harness("dests.pdf", DESTS)
}

/// `pdf` open with the Bookmarks panel showing, once the window's layout holds still.
fn bookmarks_harness(name: &'static str, pdf: &'static [u8]) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes(name, None, pdf.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "bookmarks").unwrap();
        app
    });
    // The Bookmarks panel widens to fit its titles over many frames; fit-width zoom follows it.
    // Positions are checked once the window's layout holds still.
    let mut width = 0.0;
    for _ in 0..400 {
        h.run_steps(1);
        let w = h.state().views[0].viewport_rect().width();
        if w == width {
            break;
        }
        width = w;
    }
    h
}

/// Where the point a fraction (`fx`, `fy`) across and down `page` is on screen, relative to the
/// viewport's top-left corner.
fn from_viewport_corner(h: &Harness<'static, PdfCraftApp>, page: usize, fx: f32, fy: f32) -> egui::Vec2 {
    let (r, vp) = (rect(h, page).expect("page on screen"), h.state().views[0].viewport_rect());
    egui::pos2(r.min.x + fx * r.width(), r.min.y + fy * r.height()) - vp.min
}

#[test]
fn bookmark_destinations_scroll_to_their_position() {
    use egui_kittest::kittest::Queryable;
    use pdfcraft_ui_egui::canvas::Fit;
    let mut h = dests_harness();
    // Fit width: the pages are taller than the window, so any point of them can reach its top.
    h.get_by_label("Section 1.1 Shapes").click();
    h.run_steps(6);
    let v = &h.state().views[0];
    assert_eq!(v.current, 1, "the bookmark targets page 2");
    assert_eq!(v.fit, Fit::Width, "/XYZ with zoom 0 keeps the zoom");
    let d = from_viewport_corner(&h, 1, 0.0, 0.2);
    assert!(d.y.abs() < 2.0, "y 420 on page 2 is at the top of the window, not the page top: {d:?}");

    // /XYZ left top zoom: 300%, with (125, 300), a quarter across and half way down, at the
    // window's top-left (the page is now wider than the window): its left is the edge of the
    // gutter that keeps pages clear of the quick-action bar.
    h.get_by_label("Zoomed in").click();
    h.run_steps(6);
    let v = &h.state().views[0];
    assert_eq!((v.current, v.fit, v.zoom), (1, Fit::None, 3.0));
    let d = from_viewport_corner(&h, 1, 0.25, 0.5);
    assert!((d.x - 70.0).abs() < 2.0 && d.y.abs() < 2.0, "the destination point is at the window's top-left: {d:?}");

    // Unusable operands are read as null, which keeps the reader's place in user space: y 300
    // was at the window's top on page 2 (its crop box starts at y 100), so on page 3 (crop from
    // 0) y 300 is there too, a quarter down, at the current zoom.
    h.get_by_label("Broken").click();
    h.run_steps(6);
    let v = &h.state().views[0];
    assert_eq!((v.current, v.zoom), (2, 3.0));
    let d = from_viewport_corner(&h, 2, 0.0, 0.25);
    assert!(d.y.abs() < 2.0, "y 300 on page 3 is at the top of the window: {d:?}");
}

#[test]
fn link_destinations_scroll_to_their_position() {
    use pdfcraft_ui_egui::canvas::Fit;
    let mut h = dests_harness();
    h.state_mut().set_option("zoom", "100%").unwrap();
    h.run_steps(4);
    let r = rect(&h, 0).expect("page 1");
    // The link spans x 50..250, y 300..350 in PDF space on the 300×400 page 1.
    let at = egui::pos2(r.min.x + r.width() * (150.0 / 300.0), r.min.y + r.height() * (1.0 - 325.0 / 400.0));
    h.hover_at(at);
    h.run_steps(2);
    h.drag_at(at); // press
    h.drop_at(at); // release
    h.run_steps(8);
    let v = &h.state().views[0];
    assert_eq!((v.current, v.fit), (1, Fit::Width), "/FitH goes to page 2 and fits its width");
    let d = from_viewport_corner(&h, 1, 0.0, 0.2);
    assert!(d.y.abs() < 2.0, "y 420 on page 2 is at the top of the window: {d:?}");
}

/// Two 300×400 pages. A bookmark shows page 1 at 400% with (40, 360) at the window's top-left;
/// the link there goes to page 2 as CAD exports write their cross-references, `/XYZ null null 0`.
const KEEP_PLACE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /Outlines 10 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /Annots [20 0 R] >> endobj
4 0 obj << /Type /Page /Parent 2 0 R >> endobj
10 0 obj << /Type /Outlines /First 11 0 R /Last 11 0 R /Count 1 >> endobj
11 0 obj << /Title (Start) /Parent 10 0 R /Dest [3 0 R /XYZ 40 360 4] >> endobj
20 0 obj << /Type /Annot /Subtype /Link /Rect [60 320 140 350] /Border [0 0 0] /Dest [4 0 R /XYZ null null 0] >> endobj
trailer << /Root 1 0 R >>
%%EOF";

#[test]
fn a_link_without_a_position_keeps_the_place_on_the_next_page() {
    let mut h = bookmarks_harness("keep-place.pdf", KEEP_PLACE);
    h.get_by_label("Start").click();
    h.run_steps(6);
    assert_eq!((h.state().views[0].current, h.state().views[0].zoom), (0, 4.0));
    let (fx, fy) = (40.0 / 300.0, 40.0 / 400.0);
    let d = from_viewport_corner(&h, 0, fx, fy);
    assert!((d.x - 70.0).abs() < 2.0 && d.y.abs() < 2.0, "(40, 360) is at the window's top-left: {d:?}");

    let r = rect(&h, 0).expect("page 1");
    let at = egui::pos2(r.min.x + r.width() * (100.0 / 300.0), r.min.y + r.height() * (1.0 - 335.0 / 400.0));
    h.hover_at(at);
    h.run_steps(2);
    h.drag_at(at); // press
    h.drop_at(at); // release
    h.run_steps(8);
    let v = &h.state().views[0];
    assert_eq!((v.current, v.zoom), (1, 4.0), "the link goes to page 2 at the same zoom");
    let d = from_viewport_corner(&h, 1, fx, fy);
    assert!((d.x - 70.0).abs() < 2.0 && d.y.abs() < 2.0, "page 2 shows the same place, not its top: {d:?}");
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
        app.set_option("language", "en").unwrap();
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
    // Highlighting is on by default; start from plain fields.
    h.state_mut().set_option("fields", "off").unwrap();
    crate::support::settle(&mut h);
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

/// A text field that can be filled and one that is read-only (`/Ff` 1).
const LOCKED: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R] >> >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R] >> endobj
4 0 obj << /Type /Annot /Subtype /Widget /FT /Tx /T (open) /Rect [50 300 250 330] /P 3 0 R >> endobj
5 0 obj << /Type /Annot /Subtype /Widget /FT /Tx /T (locked) /Ff 1 /Rect [50 200 250 230] /P 3 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// Highlight fields marks the fields that can be filled: a read-only one (a form's locked
/// box, or one its scripts have not opened yet) is left as it is drawn, as in Acrobat.
#[test]
fn highlight_fields_leaves_read_only_fields_alone() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("locked.pdf", None, LOCKED.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "none").unwrap();
        app.set_option("fields", "on").unwrap();
        app
    });
    h.run_steps(6);
    let tint = egui::Color32::from_rgba_unmultiplied(0x6E, 0x8E, 0xF5, 48);
    let tinted = h.output().shapes.iter().filter(|c| matches!(&c.shape, egui::Shape::Rect(r) if r.fill == tint)).count();
    assert_eq!(tinted, 1, "only the field that can be filled is tinted");
}

/// A radio group whose second button is locked on its own widget (`/Ff` 1 there), as the
/// layout of an XFA exclusion group with one read-only button writes it.
const LOCKED_RADIO: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] >> >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /Annots [5 0 R 6 0 R] >> endobj
4 0 obj << /FT /Btn /Ff 49152 /T (choice) /V /Off /Kids [5 0 R 6 0 R] >> endobj
5 0 obj << /Type /Annot /Subtype /Widget /Parent 4 0 R /Rect [50 300 90 340] /AS /Off /AP << /N << /one << >> /Off << >> >> >> /P 3 0 R >> endobj
6 0 obj << /Type /Annot /Subtype /Widget /Parent 4 0 R /Ff 1 /Rect [150 300 190 340] /AS /Off /AP << /N << /two << >> /Off << >> >> >> /P 3 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// A read-only button of a radio group is left untinted while the group's other button, which
/// can be chosen, is tinted.
#[test]
fn highlight_fields_leaves_a_groups_read_only_button_alone() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("radio.pdf", None, LOCKED_RADIO.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "none").unwrap();
        app.set_option("fields", "on").unwrap();
        app
    });
    h.run_steps(6);
    let tint = egui::Color32::from_rgba_unmultiplied(0x6E, 0x8E, 0xF5, 48);
    let tinted = h.output().shapes.iter().filter(|c| matches!(&c.shape, egui::Shape::Circle(r) if r.fill == tint)).count();
    assert_eq!(tinted, 1, "only the button that can be chosen is tinted");
}

/// A radio group's read-only button can't be chosen by a click, while the group's other button
/// can.
#[test]
fn a_groups_read_only_button_is_not_chosen_by_a_click() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("radio.pdf", None, LOCKED_RADIO.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "none").unwrap();
        app
    });
    h.run_steps(6);
    let r = rect(&h, 0).expect("page 1");
    // The buttons' centers on the 300×400 pt page (y up).
    let at = |x: f32, y: f32| egui::pos2(r.min.x + r.width() * (x / 300.0), r.min.y + r.height() * (1.0 - y / 400.0));
    let click = |h: &mut Harness<'static, PdfCraftApp>, pos: egui::Pos2| {
        h.event(egui::Event::PointerMoved(pos));
        h.run_steps(1);
        h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
        h.run_steps(1);
        h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
        h.run_steps(4);
    };
    let value = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name == "choice").map(|f| f.value.clone()).unwrap_or_default()
    };
    click(&mut h, at(70.0, 320.0));
    assert_eq!(value(&h), ["one"], "the button that can be chosen is");
    click(&mut h, at(170.0, 320.0));
    assert_eq!(value(&h), ["one"], "the read-only button leaves the choice as it was");
}

/// A required radio group (`/Ff` 32768 radio + 2 required) with two 40 pt round buttons.
const RADIOS: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] >> >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /Annots [5 0 R 6 0 R] >> endobj
4 0 obj << /FT /Btn /Ff 32770 /T (choice) /V /Off /Kids [5 0 R 6 0 R] >> endobj
5 0 obj << /Type /Annot /Subtype /Widget /Parent 4 0 R /Rect [50 300 90 340] /AS /Off /P 3 0 R >> endobj
6 0 obj << /Type /Annot /Subtype /Widget /Parent 4 0 R /Rect [150 300 190 340] /AS /Off /P 3 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// #260 (page 8): with fields highlighted, a required radio button gets a round red border, as
/// Acrobat draws it, not a square one around its corners.
#[test]
fn required_radio_buttons_get_a_round_red_border() {
    let _gpu = gpu();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("radios.pdf", None, RADIOS.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "none").unwrap();
        app.set_option("fields", "on").unwrap();
        app
    });
    crate::support::settle(&mut h);
    let r = rect(&h, 0).expect("page 1");
    let ppp = h.ctx.pixels_per_point();
    let img = h.render().expect("renders");
    // The first button spans x 50..90, y 300..340 on the 300×400 pt page (y up).
    let at = |x: f32, y: f32| {
        let p = egui::pos2(r.min.x + r.width() * (x / 300.0), r.min.y + r.height() * (1.0 - y / 400.0));
        *img.get_pixel((p.x * ppp) as u32, (p.y * ppp) as u32)
    };
    // Fit width is about 4 px in a point, and the stroke is 2 px. Chrome width can shift
    // that stroke by a fraction of a pixel, so one sample may land on its antialiased edge
    // (still red, mixed with the field tint) instead of the solid stroke.
    let stroke = |p: image::Rgba<u8>| p[0] > 180 && p[2] < 140 && i16::from(p[0]) - i16::from(p[2]) > 80;
    assert!(
        [0.05_f32, 0.2].into_iter().any(|dx| stroke(at(50.0 + dx, 320.0))),
        "the left of the circle is red: {:?} {:?}",
        at(50.05, 320.0),
        at(50.2, 320.0)
    );
    assert!(!stroke(at(50.3, 339.7)), "the widget's corner, outside the circle, is not: {:?}", at(50.3, 339.7));
}

/// Two required check boxes: the first laid out for a round XFA check button (`/PCXfaRound`), the
/// second an ordinary square one.
const ROUND_CHECKS: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R] >> >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R] >> endobj
4 0 obj << /Type /Annot /Subtype /Widget /FT /Btn /Ff 2 /T (round) /V /Off /AS /Off /Rect [50 300 90 340] /PCXfaRound true /P 3 0 R >> endobj
5 0 obj << /Type /Annot /Subtype /Widget /FT /Btn /Ff 2 /T (square) /V /Off /AS /Off /Rect [150 300 190 340] /P 3 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// With fields highlighted, a round XFA check box gets a round border, as a radio button does,
/// while an ordinary check box keeps a square one.
#[test]
fn round_check_boxes_get_a_round_highlight() {
    let _gpu = gpu();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("checks.pdf", None, ROUND_CHECKS.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "none").unwrap();
        app.set_option("fields", "on").unwrap();
        app
    });
    crate::support::settle(&mut h);
    let r = rect(&h, 0).expect("page 1");
    let ppp = h.ctx.pixels_per_point();
    let img = h.render().expect("renders");
    let at = |x: f32, y: f32| {
        let p = egui::pos2(r.min.x + r.width() * (x / 300.0), r.min.y + r.height() * (1.0 - y / 400.0));
        *img.get_pixel((p.x * ppp) as u32, (p.y * ppp) as u32)
    };
    // The red of the required border, or its antialiased edge over the field tint.
    let stroke = |p: image::Rgba<u8>| p[0] > 180 && p[2] < 140 && i16::from(p[0]) - i16::from(p[2]) > 80;
    assert!([0.05_f32, 0.2].into_iter().any(|dx| stroke(at(50.0 + dx, 320.0))), "the round box's left edge is red: {:?}", at(50.2, 320.0));
    // Near the top of the left edge: outside the circle, on a square's border.
    assert!(!stroke(at(50.25, 339.0)), "the round box's corner, outside the circle, is not: {:?}", at(50.25, 339.0));
    assert!(
        [0.1_f32, 0.25, 0.4].into_iter().any(|dx| stroke(at(150.0 + dx, 339.0))),
        "the square box's is: {:?} {:?}",
        at(150.1, 339.0),
        at(150.25, 339.0)
    );
}

/// Acrobat's Highlight existing fields is a preference, not a per-document choice: turned off
/// once, the next document (and the next session) opens with fields plain. New settings start
/// with it on.
#[test]
fn field_highlighting_is_on_by_default_and_remembered() {
    use egui_kittest::kittest::Queryable;
    let mut h = form_harness();
    h.state_mut().set_option("panel", "none").unwrap();
    h.run_steps(4);
    assert!(h.state().views[0].highlight_fields, "on by default");
    h.get_by_label("Hide field highlights").click();
    h.run_steps(4);
    assert!(!h.state().views[0].highlight_fields);
    h.state_mut().open_bytes("radios.pdf", None, RADIOS.to_vec()).expect("opens");
    h.run_steps(4);
    assert!(!h.state().views[1].highlight_fields, "the next document opens plain");
    let saved = h.state().persist();
    let mut next = PdfCraftApp::new();
    next.set_option("language", "en").unwrap();
    next.restore(&saved);
    next.open_bytes("form.pdf", None, FORM.to_vec()).expect("opens");
    assert!(!next.views[0].highlight_fields, "and so does the next session");
}

/// Settings saved before the default changed keep their stored choice; a missing or malformed
/// value falls back to the default instead of failing.
#[test]
fn stored_field_highlight_choice_wins_over_the_default() {
    for (stored, expected) in [
        (r#"{"highlight_fields": false}"#, false),
        (r#"{"highlight_fields": true}"#, true),
        (r#"{}"#, true),
        (r#"{"highlight_fields": "no"}"#, true),
        (r#"{"highlight_fields": null}"#, true),
    ] {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.restore(stored);
        app.open_bytes("form.pdf", None, FORM.to_vec()).expect("opens");
        assert_eq!(app.views[0].highlight_fields, expected, "{stored}");
    }
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
fn the_hand_tool_moves_the_page_with_the_pointer() {
    // With the Hand tool the page stays under the pointer, point for point, in the frame the
    // pointer moves (#924, #1992). The drag used to reach the scroll offset only after the frame
    // was drawn (and, before the page view stopped animating its scrolls, eased in over several
    // frames), so the page trailed behind the pointer; the points moved before egui tells a drag
    // from a click were lost on top.
    let mut h = harness_stepping(1.0 / 60.0, &[("layout", "continuous"), ("zoom", "600"), ("quick", "hand")]);
    let start = h.state().views[0].viewport_rect().center();
    h.hover_at(start);
    h.run_steps(2);
    let page = |h: &Harness<'static, PdfCraftApp>| rect(h, 0).expect("page 1 on screen").min;
    let grabbed = page(&h);
    let click_dist = h.ctx.options(|o| o.input_options.max_click_dist);
    let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    h.event(button(start, true));
    h.step();
    // A slow start, two points a frame, then quick strokes both ways. Once the pointer is further
    // than a click may wander, the page has moved exactly as far as it, in every frame.
    let mut pointer = start;
    let strokes = [egui::vec2(-2.0, -2.0); 5].into_iter().chain([egui::vec2(-40.0, -150.0), egui::vec2(25.0, 60.0)]);
    for stroke in strokes {
        pointer += stroke;
        h.event(egui::Event::PointerMoved(pointer));
        h.step();
        if (pointer - start).length() > click_dist {
            assert_eq!(page(&h) - grabbed, pointer - start, "the page keeps up with the pointer in the same frame");
        }
    }
    // Held still, it stays: nothing is left to ease in.
    h.run_steps(30);
    assert_eq!(page(&h) - grabbed, pointer - start, "nothing moves the page once the pointer stops");
    // Let go mid-stroke, it stops with the pointer: the Hand tool doesn't fling the page on.
    pointer += egui::vec2(0.0, -30.0);
    h.event(egui::Event::PointerMoved(pointer));
    h.step();
    h.event(button(pointer, false));
    h.run_steps(30);
    assert_eq!(page(&h) - grabbed, pointer - start, "the page stays where it was let go");
}

#[test]
fn required_fields_get_a_red_border_when_highlighting() {
    let _gpu = gpu();
    let mut h = form_harness();
    h.state_mut().set_option("panel", "none").unwrap();
    h.state_mut().set_option("fields", "on").unwrap();
    let props = pdfcraft_engine::FieldProps { required: Some(true), ..Default::default() };
    h.state_mut().apply_edit(pdfcraft_engine::Edit::SetFieldProps { name: "fullname".into(), props: Box::new(props) });
    crate::support::settle(&mut h);
    let r = rect(&h, 0).expect("page 1");
    // Just inside the left edge of the field (x 50, y 300..330 on a 300×400 page).
    let ppp = h.ctx.pixels_per_point();
    let at = egui::pos2(r.min.x + r.width() * (50.0 / 300.0) + 0.75, r.min.y + r.height() * (1.0 - 315.0 / 400.0));
    let img = h.render().expect("renders");
    let px = *img.get_pixel((at.x * ppp) as u32, (at.y * ppp) as u32);
    assert!(px[0] > 180 && px[1] < 100 && px[2] < 100, "a red border: {px:?}");
}

/// The page raster is shown texel for texel. At a zoom whose device scale isn't on the old 1/64
/// grid (87 % at 1 px per point: 1.16, rendered at 1.15625) the page was rendered at the
/// quantized scale and stretched onto a rectangle that didn't start on a whole pixel, so every
/// line and glyph was resampled and looked soft (#260, page 9).
#[test]
fn page_raster_maps_one_to_one_onto_screen_pixels() {
    let _gpu = gpu();
    // Fine vertical and horizontal lines (0.5 pt every 1.7 pt) fill most of a 300×400 pt page.
    let mut content = String::from("0 g\n");
    for i in 0..140 {
        let v = 30.0 + 1.7 * i as f32;
        content.push_str(&format!("{v} 40 0.5 320 re 30 {v} 240 0.5 re\n"));
    }
    content.push_str("f\n");
    let pdf = format!(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 4 0 R >> endobj
4 0 obj << /Length {} >> stream
{content}
endstream endobj
trailer << /Root 1 0 R >>
%%EOF",
        content.len()
    )
    .into_bytes();
    for ppp in [1.0, 2.0] {
        let bytes = pdf.clone();
        let mut h = Harness::builder().with_size(egui::vec2(1000.0, 800.0)).with_pixels_per_point(ppp).build_eframe(move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.open_bytes("lines.pdf", None, bytes.clone()).expect("opens");
            app.set_option("left", "closed").unwrap();
            app.set_option("panel", "none").unwrap();
            app.set_option("zoom", "87").unwrap();
            app
        });
        let (mean, worst) = compare_page_with_raster(&mut h, &pdf, 0.87);
        assert!(mean < 1.0 && worst < 16, "{ppp} px/pt: the page on screen differs from its raster: mean {mean:.2}, max {worst}");
    }
}

/// Mean and largest difference between page 1 on screen and the page rendered directly at the
/// view's exact device scale, over the lined area of `page_raster_maps_one_to_one_onto_screen_pixels`.
fn compare_page_with_raster(h: &mut Harness<'static, PdfCraftApp>, pdf: &[u8], zoom: f32) -> (f64, u8) {
    crate::support::settle(h);
    let ppp = h.ctx.pixels_per_point();
    let r = rect(h, 0).expect("page 1");
    let img = h.render().expect("renders");
    let scale = zoom * 96.0 / 72.0 * ppp;
    let mut renderer = pdfcraft_render::PageRenderer::new(std::sync::Arc::new(pdf.to_vec()), pdfcraft_render::RenderConfig::default());
    let page = renderer.render(pdfcraft_render::RenderRequest {
        page: 0,
        kind: pdfcraft_render::RequestKind::Pixels,
        tile: None,
        scale,
        tag: 0,
        preview: false,
    });
    assert!(page.error.is_none(), "{:?}", page.error);
    // Compare the lined area (page x 40..270 pt, y 60..330 pt from the top), screen against raster.
    let (ox, oy) = ((r.min.x * ppp).round() as i64, (r.min.y * ppp).round() as i64);
    let (mut sum, mut worst, mut n) = (0u64, 0u8, 0u64);
    for py in (60.0 * scale) as u32..(330.0 * scale) as u32 {
        for px in (40.0 * scale) as u32..(270.0 * scale) as u32 {
            let want = page.rgba[((py * page.width + px) * 4) as usize];
            let got = img.get_pixel((ox + px as i64) as u32, (oy + py as i64) as u32)[0];
            let d = want.abs_diff(got);
            sum += u64::from(d);
            worst = worst.max(d);
            n += 1;
        }
    }
    (sum as f64 / n as f64, worst)
}

/// A 1200 pt square page of fine lines (0.5 pt every 1.7 pt): tiled at 300 %, whole-page at 150 %.
fn fine_grid_pdf() -> Vec<u8> {
    let mut content = String::from("0 g\n");
    for i in 0..670 {
        let v = 30.0 + 1.7 * i as f32;
        content.push_str(&format!("{v} 30 0.5 1140 re 30 {v} 1140 0.5 re\n"));
    }
    content.push_str("f\n");
    format!(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 1200 1200] /Contents 4 0 R >> endobj
4 0 obj << /Length {} >> stream
{content}
endstream endobj
trailer << /Root 1 0 R >>
%%EOF",
        content.len()
    )
    .into_bytes()
}

fn grid_harness(pdf: Vec<u8>, zoom: &str) -> Harness<'static, PdfCraftApp> {
    Harness::builder().with_size(egui::vec2(1000.0, 800.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("grid.pdf", None, pdf.clone()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "none").unwrap();
        app.set_option("zoom", zoom).unwrap();
        app
    })
}

/// Detail in the middle of the window: the summed difference between neighbouring pixels.
fn centre_detail(h: &mut Harness<'static, PdfCraftApp>) -> u64 {
    let img = h.render().expect("renders");
    let (cx, cy) = (img.width() / 2, img.height() / 2);
    let mut sum = 0u64;
    for y in cy - 100..cy + 100 {
        for x in cx - 150..cx + 150 {
            sum += u64::from(img.get_pixel(x, y)[0].abs_diff(img.get_pixel(x + 1, y)[0]));
        }
    }
    sum
}

#[test]
fn zooming_a_tiled_page_shows_its_earlier_tiles_until_new_ones_arrive() {
    let _gpu = gpu();
    let mut h = grid_harness(fine_grid_pdf(), "300");
    crate::support::settle(&mut h);
    let sharp = centre_detail(&mut h);
    // The first frame at a new zoom has no tiles for it yet (they are requested at its end).
    h.state_mut().set_option("zoom", "310").unwrap();
    h.step();
    let first = centre_detail(&mut h);
    assert!(first * 10 > sharp * 7, "the first frame after zooming lost its detail: {first} against {sharp}");
}

/// #2196: zooming away from a tiled page and back must not re-blur an area already rendered
/// sharp. The tiles of the zoom just left used to be dropped while the page was shown whole,
/// so returning to it was a cold miss and every tile cost a full page interpret again.
#[test]
fn returning_to_a_visited_zoom_reuses_its_cached_tiles() {
    let _gpu = gpu();
    let mut h = grid_harness(fine_grid_pdf(), "300");
    crate::support::settle(&mut h);
    let before = h.state().views[0].tiles_debug(0);
    assert!(!before.is_empty(), "the 300 % page should be tiled, but it cached no tiles");
    // Zoom right out, where the 1200 pt page fits in one raster and is not tiled at all, so
    // none of its tiles is demanded...
    h.state_mut().set_option("zoom", "50").unwrap();
    crate::support::settle(&mut h);
    // ...then back to the first zoom, which was already rendered sharp.
    h.state_mut().set_option("zoom", "300").unwrap();
    h.step();
    let after = h.state().views[0].tiles_debug(0);
    // Every tile rendered sharp before the round trip is still cached: zooming away must not
    // drop them, so returning shows them at once instead of re-interpreting the page per tile.
    let lost: Vec<_> = before.iter().copied().filter(|k| !after.contains(k)).collect();
    assert!(lost.is_empty(), "returning to a visited zoom dropped its cached tiles {lost:?} (had {before:?}, now {after:?})");
}

#[test]
fn sidebar_collapse_and_resize_preserves_scroll_position() {
    use pdfcraft_ui_egui::canvas::Fit;
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("pages.pdf", None, PAGES.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "bookmarks").unwrap();
        app.set_option("layout", "continuous").unwrap();
        app
    });
    let mut width = 0.0;
    for _ in 0..400 {
        h.run_steps(1);
        let w = h.state().views[0].viewport_rect().width();
        if w == width {
            break;
        }
        width = w;
    }
    h.state_mut().views[0].go_to_page(2);
    h.run_steps(6);

    let v = &h.state().views[0];
    assert_eq!(v.current, 2);
    assert_eq!(v.fit, Fit::Width);
    let zoom_before = v.zoom;
    let rect_before = v.page_screen_rect(2).expect("page 2 on screen");
    let vp_before = v.viewport_rect();
    let rel_y_before = rect_before.min.y - vp_before.min.y;

    // Collapse bookmarks panel: available width increases, so fit-width zoom increases.
    h.state_mut().set_option("panel", "none").unwrap();
    h.run_steps(6);

    let v = &h.state().views[0];
    let zoom_after = v.zoom;
    assert!(zoom_after > zoom_before, "zoom increased when panel collapsed: {zoom_after} vs {zoom_before}");
    assert_eq!(v.current, 2, "still on page 2 after panel collapsed");
    let rect_after = v.page_screen_rect(2).expect("page 2 still on screen");
    let vp_after = v.viewport_rect();
    let rel_y_after = rect_after.min.y - vp_after.min.y;
    assert!((rel_y_after - rel_y_before).abs() < 1.0, "page 2 scroll position preserved: before {rel_y_before}, after {rel_y_after}");

    // Reopen bookmarks panel: available width decreases, so fit-width zoom decreases.
    h.state_mut().set_option("panel", "bookmarks").unwrap();
    for _ in 0..400 {
        h.run_steps(1);
        let w = h.state().views[0].viewport_rect().width();
        if w == width {
            break;
        }
        width = w;
    }
    let v = &h.state().views[0];
    assert_eq!(v.current, 2, "still on page 2 after panel reopened");
    let rect_reopened = v.page_screen_rect(2).expect("page 2 still on screen");
    let vp_reopened = v.viewport_rect();
    let rel_y_reopened = rect_reopened.min.y - vp_reopened.min.y;
    assert!((rel_y_reopened - rel_y_before).abs() < 1.0, "page 2 scroll position preserved on reopen: before {rel_y_before}, after {rel_y_reopened}");
}

/// Synthetic unequal-height spreads, generated without font or image assets.
fn final_spread_at_scroll_end(heights: &[u32], cover: bool) -> (Vec<(usize, egui::Rect)>, egui::Rect) {
    let kids = (0..heights.len()).map(|i| format!("{} 0 R", i + 3)).collect::<Vec<_>>().join(" ");
    let mut pdf = format!(
        "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [{kids}] /Count {} >> endobj\n",
        heights.len()
    );
    for (i, height) in heights.iter().enumerate() {
        pdf += &format!("{} 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 {height}] >> endobj\n", i + 3);
    }
    pdf += "trailer << /Root 1 0 R >>\n%%EOF";
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("unequal-spreads.pdf", None, pdf.as_bytes().to_vec()).unwrap();
        for (key, value) in [("left", "closed"), ("panel", "none"), ("layout", "two-up"), ("zoom", "200")] {
            app.set_option(key, value).unwrap();
        }
        app.set_option("cover", if cover { "on" } else { "off" }).unwrap();
        app
    });
    h.run_steps(12);
    let at = h.state().views[0].viewport_rect().center();
    h.hover_at(at);
    h.run_steps(2);
    for _ in 0..4 {
        h.event(wheel(MouseWheelUnit::Line, -1000.0, TouchPhase::Move));
        h.run_steps(45);
    }
    let view = &h.state().views[0];
    let last = heights.len() - 1;
    let first = if cover { last - (last + 1) % 2 } else { last - last % 2 };
    let pages = (first..=last).map(|page| (page, view.page_screen_rect(page).expect("final spread is visible"))).collect();
    (pages, view.viewport_rect())
}

fn assert_final_spread_bottom_reachable(heights: &[u32], cover: bool) {
    let (pages, viewport) = final_spread_at_scroll_end(heights, cover);
    let bottom = pages.iter().map(|(_, rect)| rect.bottom()).fold(0.0_f32, f32::max);
    assert!(
        bottom <= viewport.bottom() + 1.0,
        "final spread remains below the viewport: heights={heights:?}, cover={cover}, pages={pages:?}, viewport={viewport:?}"
    );
    // The maximum-height page reaches the bottom margin after the scroll offset settles.
    assert!((viewport.bottom() - bottom - 28.0).abs() < 1.0, "scroll end leaves the bottom margin: pages={pages:?}, viewport={viewport:?}");
}

#[test]
fn two_up_scroll_end_reaches_a_taller_left_page() {
    assert_final_spread_bottom_reachable(&[400, 400, 400, 100], false);
}

#[test]
fn two_up_scroll_end_reaches_a_taller_right_page() {
    assert_final_spread_bottom_reachable(&[400, 400, 100, 400], false);
}

#[test]
fn two_up_scroll_end_reaches_equal_height_pages() {
    assert_final_spread_bottom_reachable(&[400, 400, 400, 400], false);
}

#[test]
fn two_up_scroll_end_reaches_an_unpaired_final_page() {
    assert_final_spread_bottom_reachable(&[400, 400, 400], false);
}

#[test]
fn two_up_scroll_end_with_cover_reaches_a_taller_left_page() {
    assert_final_spread_bottom_reachable(&[400, 400, 400, 400, 100], true);
}

#[test]
fn switching_modes_keeps_the_current_page_at_fixed_zoom() {
    use egui_kittest::kittest::Queryable;
    let kids = (3..33).map(|i| format!("{i} 0 R")).collect::<Vec<_>>().join(" ");
    let mut pdf = format!(
        "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [{kids}] /Count 30 /MediaBox [0 0 300 400] >> endobj\n"
    );
    for i in 3..33 {
        pdf.push_str(&format!("{i} 0 obj << /Type /Page /Parent 2 0 R >> endobj\n"));
    }
    pdf.push_str("trailer << /Root 1 0 R >>\n%%EOF");
    let other = pdf.clone().into_bytes();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("pages.pdf", None, pdf.into_bytes()).unwrap();
        app.set_option("zoom", "100").unwrap();
        app
    });
    h.run_steps(8);
    h.state_mut().views[0].go_to_page(17);
    h.run_steps(8);
    assert_eq!(h.state().views[0].current, 17);
    h.hover_at(h.state().views[0].viewport_rect().center());
    h.event(wheel(MouseWheelUnit::Point, -35.0, TouchPhase::Move));
    h.run_steps(8);
    let before = rect(&h, 17).unwrap().top() - h.state().views[0].viewport_rect().top();
    h.state_mut().run_command("view.read_mode");
    h.run_steps(8);
    assert_eq!(h.state().views[0].current, 17, "switching to Read should keep the current page");
    let after = rect(&h, 17).unwrap().top() - h.state().views[0].viewport_rect().top();
    assert!((before - after).abs() < 1.0, "the position within the page moved: {before} to {after}");
    h.state_mut().views[0].go_to_page(5);
    h.run_steps(8);
    assert_eq!(h.state().views[0].current, 5);
    h.get_by_label("All tools").click();
    h.run_steps(8);
    assert_eq!(h.state().views[0].current, 5, "switching to All tools should keep the current page");
    h.state_mut().open_bytes("other.pdf", None, other).unwrap();
    h.state_mut().set_option("zoom", "100").unwrap();
    h.run_steps(8);
    h.state_mut().views[1].go_to_page(9);
    h.run_steps(8);
    assert_eq!(h.state().views[1].current, 9);
    h.get_by_label("pages.pdf").click();
    h.run_steps(8);
    assert_eq!(h.state().views[0].current, 5, "the first tab keeps its own position");
    h.get_by_label("other.pdf").click();
    h.run_steps(8);
    assert_eq!(h.state().views[1].current, 9, "the second tab keeps its own position");
}

#[test]
fn long_view_menu_fits_a_short_window() {
    use egui_kittest::kittest::Queryable;
    let mut h = Harness::builder().with_size(egui::vec2(1000.0, 400.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("pages.pdf", None, PAGES.to_vec()).unwrap();
        app
    });
    h.run_steps(8);
    crate::support::open_menu(&mut h, "View");
    h.run_steps(2);
    crate::support::menu_row(&h, "Rotate view").hover();
    h.event(wheel(MouseWheelUnit::Point, -1000.0, TouchPhase::Move));
    h.run_steps(6);
    let after = crate::support::menu_row(&h, "Side panels").rect().bottom();
    assert!(after <= 400.0, "the menu scrolls Side panels into view: {after}");
    crate::support::menu_row(&h, "Side panels").hover();
    h.run_steps(4);
    h.get_all_by_label("Bookmarks").last().unwrap().click();
    h.run_steps(4);
    assert_eq!(h.state().right, Some(pdfcraft_ui_egui::RightPanel::Bookmarks));
}

fn click_with(h: &mut Harness<'static, PdfCraftApp>, pos: egui::Pos2, modifiers: Modifiers) {
    h.hover_at(pos);
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers });
    }
    h.run_steps(4);
}

/// Full screen is Acrobat's Full Screen Mode (#379, #1459): one page at a time, fitted to the
/// screen, with no tools; a click, Space, Enter or → go forward a page and a ⇧-click, Backspace
/// or ← back; Esc brings the display from before it back, at the page reached.
#[test]
fn full_screen_shows_one_fitted_page_and_clicks_turn_it() {
    use pdfcraft_ui_egui::canvas::Fit;
    let mut h = harness(&[("layout", "continuous"), ("zoom", "50")]);
    h.run_steps(4);
    assert!(h.state().views[0].continuous(), "continuous before");
    h.key_press_modifiers(Modifiers::COMMAND, Key::L);
    h.run_steps(6);
    assert!(h.state().full_screen);
    let v = &h.state().views[0];
    assert!(!v.continuous(), "one page at a time");
    assert_eq!(v.fit, Fit::Page, "fitted to the screen");
    assert!(rect(&h, 0).is_some() && rect(&h, 1).is_none(), "only page 1 is on screen");
    let r = rect(&h, 0).unwrap();
    let vp = h.state().views[0].viewport_rect();
    assert!(r.height() <= vp.height() + 1.0 && r.width() <= vp.width() + 1.0, "the page fits: {r:?} in {vp:?}");
    assert!(h.query_by_label("Select (V)").is_none(), "no quick tools in full screen");
    click_with(&mut h, r.center(), Modifiers::NONE);
    assert_eq!(h.state().views[0].current, 1, "a click goes forward a page");
    h.key_press(Key::ArrowRight);
    h.run_steps(4);
    assert_eq!(h.state().views[0].current, 2, "→ goes forward");
    h.key_press(Key::Backspace);
    h.run_steps(4);
    assert_eq!(h.state().views[0].current, 1, "Backspace goes back");
    let r = rect(&h, 1).expect("page 2 on screen");
    click_with(&mut h, r.center(), Modifiers::SHIFT);
    assert_eq!(h.state().views[0].current, 0, "a shift-click goes back");
    h.key_press(Key::Space);
    h.run_steps(4);
    assert_eq!(h.state().views[0].current, 1, "Space goes forward");
    h.key_press(Key::Escape);
    h.run_steps(4);
    assert!(!h.state().full_screen, "Escape leaves full screen");
    let v = &h.state().views[0];
    assert!(v.continuous(), "the display from before comes back");
    assert_eq!(v.fit, Fit::None);
    assert!((v.zoom - 0.5).abs() < 1e-3, "zoom back to 50%: {}", v.zoom);
    assert_eq!(v.current, 1, "at the page reached in full screen");
    h.run_steps(2);
    assert!(h.query_by_label("Select (V)").is_some(), "the quick tools are back");
}

/// #2070 review: full screen survives the file being reloaded (Esc still brings the display from
/// before back), and a document opened in full screen takes its own initial view first, so Esc
/// comes back to that and full screen stays one spread at a time.
#[test]
fn full_screen_survives_a_reload_and_keeps_a_new_documents_own_view() {
    use pdfcraft_ui_egui::canvas::{Fit, PageLayout};
    let mut h = harness(&[("layout", "continuous"), ("zoom", "50")]);
    h.run_steps(4);
    h.key_press_modifiers(Modifiers::COMMAND, Key::L);
    h.run_steps(6);
    {
        let app = h.state_mut();
        let info = app.session.get(app.views[0].id).unwrap().info.clone();
        app.views[0].document_replaced(&info);
    }
    h.run_steps(4);
    assert!(h.state().views[0].presenting(), "still presenting after the reload");
    h.key_press(Key::Escape);
    h.run_steps(4);
    let v = &h.state().views[0];
    assert!(v.continuous() && v.fit == Fit::None && (v.zoom - 0.5).abs() < 1e-3, "the display from before comes back");

    h.key_press_modifiers(Modifiers::COMMAND, Key::L);
    h.run_steps(6);
    let two_columns = String::from_utf8_lossy(PAGES).replace("/Type /Catalog /Pages 2 0 R", "/Type /Catalog /Pages 2 0 R /PageLayout /TwoColumnLeft");
    h.state_mut().open_bytes("two.pdf", None, two_columns.into_bytes()).expect("opens");
    h.run_steps(6);
    let v = h.state().views.last().unwrap();
    assert!(v.presenting() && v.layout == PageLayout::TwoUp && !v.continuous(), "one spread at a time");
    h.key_press(Key::Escape);
    h.run_steps(4);
    let v = h.state().views.last().unwrap();
    assert!(v.layout == PageLayout::TwoUp && v.continuous(), "the document's own two-column view comes back");
}

/// Two-page view stays a spread in full screen (#1353): both pages of the spread, fitted, and
/// → turns the spread.
#[test]
fn full_screen_keeps_two_page_view_as_spreads() {
    use pdfcraft_ui_egui::canvas::PageLayout;
    let mut h = harness(&[("layout", "two-up")]);
    h.run_steps(4);
    h.key_press_modifiers(Modifiers::COMMAND, Key::L);
    h.run_steps(6);
    let v = &h.state().views[0];
    assert_eq!(v.layout, PageLayout::TwoUp);
    assert!(!v.continuous(), "one spread at a time");
    assert!(rect(&h, 0).is_some() && rect(&h, 1).is_some() && rect(&h, 2).is_none(), "the first spread only");
    h.key_press(Key::ArrowRight);
    h.run_steps(4);
    assert_eq!(h.state().views[0].current, 2, "→ turns the spread");
    assert!(rect(&h, 2).is_some() && rect(&h, 0).is_none());
    h.key_press(Key::Escape);
    h.run_steps(4);
    let v = &h.state().views[0];
    assert!(v.continuous() && v.layout == PageLayout::TwoUp, "two-page continuous view comes back");
}

/// Review on #2070: full screen hides the tools and a click turns the page only with Select, so
/// entering it from another tool selects Select, and leaving gives the tool back.
#[test]
fn full_screen_clicks_turn_pages_whatever_tool_was_on() {
    use pdfcraft_ui_egui::QuickTool;
    let mut h = harness(&[]);
    h.run_steps(4);
    h.state_mut().quick_tool = QuickTool::Hand;
    h.key_press_modifiers(Modifiers::COMMAND, Key::L);
    h.run_steps(6);
    assert!(h.state().full_screen);
    assert_eq!(h.state().quick_tool, QuickTool::Select, "Select in full screen");
    // Entering again (a second document asking for it) keeps the tool to give back.
    h.state_mut().apply_full_screen(true);
    let r = rect(&h, 0).expect("page 1 on screen");
    click_with(&mut h, r.center(), Modifiers::NONE);
    assert_eq!(h.state().views[0].current, 1, "a click goes forward");
    h.key_press(Key::Escape);
    h.run_steps(4);
    assert!(!h.state().full_screen);
    assert_eq!(h.state().quick_tool, QuickTool::Hand, "the tool from before is back");
}

/// #4415: the Pages panel's Page tools menu. Each tool acts on the selected pages (the current
/// page when none is): rotate, delete and insert a blank page here, as Pages ▸ … does.
#[test]
fn the_pages_panel_tools_act_on_the_selected_pages() {
    let mut h = harness(&[("panel", "pages")]);
    let id = h.state().views[0].id;
    let rotations = |h: &Harness<'static, PdfCraftApp>| h.state().session.get(id).unwrap().info.pages.iter().map(|p| p.rotation).collect::<Vec<_>>();
    // The panel's Page tools menu, on the right-hand side of the window.
    let tool = |h: &mut Harness<'static, PdfCraftApp>, label: &str| {
        h.get_all_by_label("Page tools").find(|n| n.rect().left() > 1000.0).expect("Page tools in the Pages panel").click();
        h.run_steps(2);
        h.get_by_label(label).click();
        h.run_steps(3);
    };
    h.state_mut().views[0].select_pages(&[1, 2]);
    h.run_steps(2);
    tool(&mut h, "Rotate the selected pages clockwise");
    assert_eq!(rotations(&h), [0, 90, 90], "pages 2 and 3, the selection");
    tool(&mut h, "Rotate the selected pages counterclockwise");
    assert_eq!(rotations(&h), [0, 0, 0]);
    h.state_mut().views[0].select_pages(&[0]);
    h.run_steps(2);
    tool(&mut h, "Delete the selected pages");
    assert_eq!(h.state().session.get(id).unwrap().info.pages.len(), 2, "page 1 deleted");
    tool(&mut h, "Insert a blank page after the selection");
    assert_eq!(h.state().session.get(id).unwrap().info.pages.len(), 3);
    h.state_mut().execute("edit.undo");
    h.run_steps(2);
    assert_eq!(h.state().session.get(id).unwrap().info.pages.len(), 2, "each is one undo step");
}

/// CodeRabbit on #4583: the Page tools menu's file items work from the menu too. Insert pages
/// from a file puts the picked file's pages after the selection; Extract the selected pages
/// opens Extract for the selection and writes it.
#[test]
fn the_pages_panel_tools_insert_from_a_file_and_extract() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-page-tools-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let more = dir.join("more.pdf");
    std::fs::write(&more, PAGES).unwrap();
    let mut h = harness(&[("panel", "pages")]);
    let id = h.state().views[0].id;
    let count = |h: &Harness<'static, PdfCraftApp>| h.state().session.get(id).unwrap().info.pages.len();
    let tool = |h: &mut Harness<'static, PdfCraftApp>, label: &str| {
        h.get_all_by_label("Page tools").find(|n| n.rect().left() > 1000.0).expect("Page tools in the Pages panel").click();
        h.run_steps(2);
        h.get_by_label(label).click();
        h.run_steps(3);
    };
    assert_eq!(count(&h), 3);
    h.state_mut().views[0].select_pages(&[0]);
    h.state_mut().pick_override = Some(vec![more.to_string_lossy().into_owned()]);
    tool(&mut h, "Insert pages from a file after the selection…");
    h.run_steps(3);
    assert_eq!(count(&h), 6, "the picked file's three pages are inserted");

    let out = dir.join("out");
    std::fs::create_dir_all(&out).unwrap();
    h.state_mut().export_dir_override = Some(out.to_string_lossy().into_owned());
    h.state_mut().views[0].select_pages(&[1, 2]);
    tool(&mut h, "Extract the selected pages…");
    assert_eq!(h.state().dialog, Some(pdfcraft_ui_egui::Dialog::Extract), "Extract opens for the selection");
    h.state_mut().extract_draft = pdfcraft_ui_egui::ExtractDraft { separate: true, ..Default::default() };
    h.get_all_by_label("Extract").last().unwrap().click();
    h.run_steps(3);
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 2, "the two selected pages, each a file");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The Pages panel's Page tools are greyed out on a document whose security doesn't allow
/// page changes: the menu doesn't open, and nothing changes.
#[test]
fn the_pages_panel_tools_are_off_without_page_editing_permission() {
    use egui_kittest::kittest::NodeT;
    let mut h = harness(&[("panel", "pages")]);
    {
        let app = h.state_mut();
        app.apply_edit(pdfcraft_engine::Edit::Protect(pdfcraft_engine::Protection {
            permissions_password: Some("owner".into()),
            changes: pdfcraft_engine::Changes::None,
            ..Default::default()
        }));
        let bytes = app.session.save_bytes(app.views[0].id).unwrap();
        app.open_bytes("restricted.pdf", None, bytes.as_ref().clone()).unwrap();
    }
    h.run_steps(6);
    let i = h.state().active.unwrap();
    let id = h.state().views[i].id;
    assert!(!h.state().session.get(id).unwrap().allows_assembly());
    let tools = h.get_all_by_label("Page tools").find(|n| n.rect().left() > 1000.0).expect("Page tools");
    assert!(tools.accesskit_node().is_disabled(), "Page tools is greyed out");
    tools.click();
    h.run_steps(3);
    assert!(h.query_by_label("Delete the selected pages").is_none(), "its menu doesn't open");
    assert_eq!(h.state().session.get(id).unwrap().info.pages.len(), 3);
}

#[test]
fn a_document_press_releases_pages_keyboard_focus_without_escape() {
    let mut h = harness(&[("panel", "pages")]);
    h.get_by_label("Page 2").click();
    h.run_steps(3);
    let focused = h.ctx.memory(|m| m.focused());
    assert!(focused.is_some(), "the thumbnail owns keyboard input");
    let pos = rect(&h, 1).unwrap().center();
    h.event(egui::Event::PointerMoved(pos));
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.run_steps(1);
    }
    h.run_steps(2);
    assert_ne!(h.ctx.memory(|m| m.focused()), focused, "pressing the document relinquishes Pages focus without Escape");
    sidebar_key(&mut h, Key::A, Modifiers::COMMAND);
    assert!(h.state().views[0].selected.is_empty(), "Ctrl+A now belongs to the viewer, not page thumbnails");
}
