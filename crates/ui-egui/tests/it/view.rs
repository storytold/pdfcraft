//! Page layouts, zoom modes and link navigation, checked by where pages land on screen.

use egui::{Key, Modifiers, MouseWheelUnit, TouchPhase};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;
use pdfcraft_ui_egui::settings::PdfCraftSettings;

/// Saved settings that hold only the Default page display `token`.
fn saved_default_layout(token: &str) -> PdfCraftSettings {
    let mut saved = PdfCraftSettings::default();
    saved.layout.default_layout = Some(token.to_owned());
    saved
}

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
use crate::support::AppHarness as _;

fn harness(options: &'static [(&'static str, &'static str)]) -> Harness<'static, PdfCraftApp> {
    harness_stepping(1.0 / 4.0, options)
}

/// [`harness`] with frames `step_dt` seconds apart (a quarter second is kittest's default).
fn harness_stepping(step_dt: f32, options: &'static [(&'static str, &'static str)]) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(step_dt).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "pages.pdf", None, PAGES.to_vec()).expect("opens");
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "panel", "none").unwrap();
            for (k, v) in options {
                app.set_option(&egui::Context::default(), k, v).unwrap();
            }
            app
        }
    });
    h.run_steps(8);
    h
}

fn rect(h: &Harness<'static, PdfCraftApp>, page: usize) -> Option<egui::Rect> {
    h.state().views[0].page_screen_rect(page)
}

/// Turns on the Preferences row `label` in the Documents and view card and applies it with OK, as a user does.
fn choose_in_documents_preferences(h: &mut Harness<'static, PdfCraftApp>, label: &str) {
    h.get_by_label(label).scroll_to_me();
    h.run_steps(4);
    h.get_by_label(label).click();
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
}

/// Picks `option` in the Preferences choice row `row` on the Documents and view page and applies it with OK, as a
/// user does: the row is a select, whose options show once it is opened.
fn pick_in_documents_preferences(h: &mut Harness<'static, PdfCraftApp>, row: &str, option: &str) {
    let group = egui::accesskit::Role::RadioGroup;
    if h.query_by_role_and_label(group, row).is_some() {
        // A short choice is a row of segments.
        h.get_by_role_and_label(group, row).scroll_to_me();
        h.run_steps(4);
        h.get_by_role_and_label(group, row).get_by_label(option).click();
        h.run_steps(2);
    } else {
        h.get_by_role_and_label(egui::accesskit::Role::ComboBox, row).scroll_to_me();
        h.run_steps(4);
        h.get_by_role_and_label(egui::accesskit::Role::ComboBox, row).click();
        h.run_steps(3);
        h.get_all_by_label(option).last().unwrap_or_else(|| panic!("the {option:?} option is on screen")).click();
        h.run_steps(2);
    }
    h.get_by_label("OK").click();
    h.run_steps(3);
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
    assert_eq!(h.state().views[0].current(), 1);
    // Moving the pointer is not a keyboard-focus change.
    h.hover_at(rect(&h, 1).unwrap().center());
    h.run_steps(1);
    for _ in 0..2 {
        sidebar_key(&mut h, Key::A, Modifiers::COMMAND);
        assert_eq!(h.state().views[0].target_pages(), vec![0, 1, 2]);
        assert_eq!(h.state().views[0].current(), 1, "selecting does not navigate");
    }
    sidebar_key(&mut h, Key::Escape, Modifiers::NONE);
    assert!(h.state().views[0].selected.items.is_empty());

    // A click on the viewer returns Ctrl+A to text selection, even with Pages open.
    let pos = rect(&h, 1).unwrap().center();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    }
    h.run_steps(3);
    sidebar_key(&mut h, Key::A, Modifiers::COMMAND);
    assert!(h.state().views[0].selected.items.is_empty(), "viewer shortcut must not select thumbnails");

    // A different document does not inherit the previous sidebar's keyboard ownership.
    h.get_by_label("Page 2").click();
    h.run_steps(2);
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "other.pdf", None, PAGES.to_vec()).unwrap();
    h.run_steps(5);
    sidebar_key(&mut h, Key::A, Modifiers::COMMAND);
    let active = h.state().active.unwrap();
    assert!(h.state().views[active].selected.items.is_empty());
    h.get_by_label("Page 1").click();
    h.run_steps(2);
    sidebar_key(&mut h, Key::A, Modifiers::COMMAND);
    assert_eq!(h.state().views[active].target_pages(), vec![0, 1, 2]);
    assert!(h.state().views[0].selected.items.is_empty(), "selection remains document-local");
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
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "mixed.pdf", None, MIXED.to_vec()).expect("opens");
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "panel", "none").unwrap();
            for (k, v) in options {
                app.set_option(&egui::Context::default(), k, v).unwrap();
            }
            app
        }
    });
    h.run_steps(8);
    h
}

/// A vertical wheel event; negative `dy` scrolls down.
fn wheel(unit: MouseWheelUnit, dy: f32, phase: TouchPhase) -> egui::Event {
    egui::Event::MouseWheel { unit, delta: egui::vec2(0.0, dy), phase, modifiers: Modifiers::NONE, source: egui::MouseWheelSource::Unknown }
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
    let zoom = h.state().views[0].zoom();
    let original = rect(&h, 1).unwrap().top() - rect(&h, 0).unwrap().bottom();
    assert!(original > 1.0);
    h.set_option("page-gaps", "off").unwrap();
    h.run_steps(4);
    for page in 0..2 {
        let (a, b) = (rect(&h, page).unwrap(), rect(&h, page + 1).unwrap());
        assert!((b.top() - a.bottom()).abs() < 0.01, "pages must touch: {a:?} {b:?}");
    }
    assert_eq!(h.state().views[0].zoom(), zoom);
    let id = h.state().views[0].id;
    assert!(!h.state().session.get(id).unwrap().dirty, "a view option must not edit the PDF");
    assert!(h.execute("view.page_gaps"));
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
    assert!(h.execute("page.organize"));
    h.run_steps(4);
    assert!(h.state().views[0].organize);
    assert!(h.execute("view.page_gaps"));
    h.run_steps(2);
    assert!(!h.state().views[0].view.page_gaps);
    assert!(h.execute("page.organize"));
    h.run_steps(4);
    let after = rect(&h, 1).unwrap();
    // Keeping the scroll offset moves page 2 up by exactly the removed gap before it.
    assert!((after.top() - before.top() + gap).abs() < 1.0, "stale grid anchoring moved the page: {before:?} -> {after:?}, gap={gap}");
    assert_eq!(h.state().views[0].current(), 1);
}

/// A three-page comparison, with the old pane leading while the new document stays active.
fn page_gaps_comparison_harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.open_bytes(&egui::Context::default(), "old.pdf", None, PAGES.to_vec()).unwrap();
            app.open_bytes(&egui::Context::default(), "new.pdf", None, PAGES.to_vec()).unwrap();
            app.set_option(&egui::Context::default(), "zoom", "50").unwrap();
            app.compare_old = Some(app.views[0].id);
            app.run_compare(&egui::Context::default());
            app
        }
    });
    h.run_steps(4);
    let at = h.state().views[0].viewport_rect().center();
    h.hover_at(at);
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.run_steps(1);
    }
    h.run_steps(2);
    assert_eq!(h.state().compare.as_ref().unwrap().views.lead, craft::pages::ui::Lead::Left);
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
    assert!(h.state().views.iter().all(|v| !v.view.page_gaps), "the old leader must not restore gaps after the active pane's menu action");
}

/// The control setting has the same synchronized behavior and leaves unrelated views alone.
#[test]
fn page_gaps_comparison_control_preserves_both_states() {
    let mut h = page_gaps_comparison_harness();
    for (value, show) in [("off", false), ("on", true)] {
        h.set_option("page-gaps", value).unwrap();
        h.run_steps(4);
        assert!(h.state().views.iter().all(|v| v.view.page_gaps == show), "comparison rejected page-gaps={value}");
    }
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "unrelated.pdf", None, PAGES.to_vec()).unwrap();
    h.set_option("page-gaps", "off").unwrap();
    assert!(h.state().views[0].view.page_gaps && h.state().views[1].view.page_gaps);
    assert!(!h.state().views[2].view.page_gaps);
}

/// The saved default survives restart and applies to newly opened documents.
#[test]
fn page_gaps_default_survives_restart_and_applies_to_new_documents() {
    let mut app = crate::control::new_app();
    app.set_option(&egui::Context::default(), "default-page-gaps", "off").unwrap();
    let mut restored = crate::control::new_app();
    restored.apply_settings(&app.settings_snapshot());
    restored.open_bytes(&egui::Context::default(), "pages.pdf", None, PAGES.to_vec()).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        restored
    });
    h.run_steps(8);
    h.set_option("zoom", "50").unwrap();
    h.run_steps(4);
    assert!((rect(&h, 1).unwrap().top() - rect(&h, 0).unwrap().bottom()).abs() < 0.01);
    assert!(h.set_option("page-gaps", "invalid").is_err());
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
    let ctx_h = h.ctx.clone();
    h.state_mut().views[0].document_replaced(&ctx_h, &info);
    assert!(!h.state().views[0].view.page_gaps, "a file reload preserves the view setting");
}

/// Page edges stay adjacent at integer and fractional physical-pixel scales.
#[test]
fn page_gaps_stay_closed_at_fractional_device_scales() {
    for ppp in [1.0, 1.25, 1.5, 2.0] {
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_pixels_per_point(ppp).build_eframe(|cc| {
            crate::support::install_runtime(&cc.egui_ctx);
            {
                let mut app = crate::control::new_app();
                app.open_bytes(&egui::Context::default(), "pages.pdf", None, PAGES.to_vec()).unwrap();
                for (key, value) in [("left", "closed"), ("panel", "none"), ("layout", "continuous"), ("zoom", "43"), ("page-gaps", "off")] {
                    app.set_option(&egui::Context::default(), key, value).unwrap();
                }
                app
            }
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
    use pdfcraft_ui_egui::canvas::PageDisplay;
    // Two-page view without continuous scrolling shows just the current spread; the continuous
    // checkbox reveals the rest (#752).
    let mut h = harness(&[("layout", "two-page"), ("zoom", "50")]);
    h.run_steps(4);
    assert_eq!(h.state().views[0].view.display, PageDisplay::TwoUp);
    assert!(!h.state().views[0].continuous(), "`two-page` is the non-scrolling two-page display");
    assert!(rect(&h, 0).is_some() && rect(&h, 1).is_some(), "the first spread is on screen");
    assert!(rect(&h, 2).is_none(), "the next spread is not shown until paged or scrolled");
    // Turning on continuous scrolling lays every spread out in one column.
    h.set_option("continuous", "on").unwrap();
    h.run_steps(4);
    assert!(rect(&h, 2).is_some(), "continuous scrolling reveals the later spreads");
    assert_eq!(h.state().views[0].view.display, PageDisplay::TwoUp, "the base layout is unchanged");
}

#[test]
fn single_page_layout_shows_one_page_at_a_time() {
    let mut h = harness(&[("layout", "single"), ("zoom", "50")]);
    h.run_steps(4);
    assert!(rect(&h, 0).is_some() && rect(&h, 1).is_none(), "only page 1");
    h.set_option("page", "2").unwrap();
    h.run_steps(4);
    assert!(rect(&h, 0).is_none() && rect(&h, 1).is_some(), "only page 2");
}

#[test]
fn single_page_jump_draws_a_fitting_page_where_it_stays() {
    // A page that fits has nothing to scroll, but the jump to it asked for the gap above it:
    // drawn that much too high for a frame, it then hopped into place.
    let mut h = harness_stepping(1.0 / 60.0, &[("layout", "single")]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num0); // fit page
    h.run_steps(4);
    h.set_option("page", "2").unwrap();
    h.step();
    let first = rect(&h, 1).expect("page 2 drawn in the frame after the jump");
    h.run_steps(4);
    let settled = rect(&h, 1).expect("page 2");
    assert!((first.min.y - settled.min.y).abs() < 0.5, "page 2 drawn where it settles: {first:?} then {settled:?}");
}

/// The 40 frames from one wheel notch on, with Preferences ▸ Animation effects as given: how far
/// page 1 has moved up in each frame, or with ⌘/Ctrl held, the view's zoom.
fn frames_after_a_notch(animation_effects: &str, zooming: bool) -> Vec<f32> {
    let mut h = harness_stepping(1.0 / 60.0, &[("layout", "continuous"), ("zoom", "100")]);
    h.set_option("animation_effects", animation_effects).unwrap();
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
            if zooming { h.state().views[0].view.zoom as f32 } else { before - rect(&h, 0).expect("page 1 still laid out").min.y }
        })
        .collect()
}

/// The Animation effects choices that scroll a notch at once and the ones that ease it. The system's
/// reduced-motion answer reaches the app through the framework's env only, so a test cannot ask for it.
const AT_ONCE: [&str; 1] = ["off"];
const EASED: [&str; 2] = ["system", "on"];

#[test]
fn a_wheel_notch_scrolls_at_once_with_animation_effects_off() {
    // Eased, the page view spreads a notch over several frames. At once, the page moves the notch's
    // whole distance in one frame.
    let notch = pdfcraft_ui_egui::scroll_speed::ScrollSpeed::default().wheel_line();
    // The first frame the page stands the whole notch further on.
    let arrives = |moved: &[f32]| moved.iter().position(|y| (y - notch).abs() < 0.5).unwrap_or(moved.len());
    let mut at_once = 0;
    for effects in AT_ONCE {
        let moved = frames_after_a_notch(effects, false);
        // The frame the notch arrives in scrolls; the next one draws the page where it stays.
        assert!(moved[2..].iter().all(|y| (y - notch).abs() < 0.5), "{effects}: one notch is {notch} points, all at once: {moved:?}");
        at_once = at_once.max(arrives(&moved));
    }
    for effects in EASED {
        let moved = frames_after_a_notch(effects, false);
        assert!(arrives(&moved) > at_once + 1, "{effects}: the notch eases over more frames than at once ({at_once}): {moved:?}");
        assert!((moved[39] - notch).abs() < 1.0, "{effects}: easing changes how long a notch takes, not how far it goes: {moved:?}");
    }
}

#[test]
fn page_display_commands_switch_layouts() {
    let mut h = harness(&[]);
    h.execute("view.layout.single");
    assert_eq!(h.state().views[0].view.display, pdfcraft_ui_egui::canvas::PageDisplay::Single);
    // The cover toggle refuses outside two-page view instead of arming a hidden flag.
    assert!(!h.state().views[0].view.cover);
    assert!(!h.execute("view.layout.cover"));
    assert!(!h.state().views[0].view.cover);
    h.execute("view.layout.two_up");
    assert_eq!(h.state().views[0].view.display, pdfcraft_ui_egui::canvas::PageDisplay::TwoUp);
    assert!(h.execute("view.layout.cover"));
    assert!(h.state().views[0].view.cover, "the cover toggle flips");
    assert!(h.execute("view.layout.cover"));
    assert!(!h.state().views[0].view.cover, "toggling twice restores");
    // Continuous scrolling is an independent toggle over the base layout (#752).
    assert!(h.state().views[0].continuous(), "the default view scrolls continuously");
    h.execute("view.continuous");
    h.run_steps(3);
    assert!(!h.state().views[0].continuous(), "the checkbox turns continuous scrolling off");
    assert_eq!(h.state().views[0].view.display, pdfcraft_ui_egui::canvas::PageDisplay::TwoUp, "toggling scrolling leaves the base layout");
    h.execute("view.layout.single");
    assert_eq!(h.state().views[0].view.display, pdfcraft_ui_egui::canvas::PageDisplay::Single);
    assert!(!h.state().views[0].continuous(), "switching the base layout leaves scrolling off");
}

#[test]
fn layout_option_rejects_typos() {
    let mut h = harness(&[]);
    assert!(h.set_option("layout", "singel").is_err());
    assert!(h.set_option("default-layout", "bogus").is_err());
    assert!(h.set_option("cover", "maybe").is_err());
    assert_eq!(h.state().views[0].view.display, pdfcraft_ui_egui::canvas::PageDisplay::Single);
    assert!(h.state().views[0].continuous(), "a rejected typo leaves the default display unchanged");
    assert!(h.set_option("cover", "on").is_err(), "cover needs two-page view first");
    h.set_option("layout", "two-up").unwrap();
    h.set_option("cover", "on").unwrap();
    assert!(h.state().views[0].view.cover);
    h.set_option("cover", "off").unwrap();
    assert!(!h.state().views[0].view.cover);
}

#[test]
fn view_options_without_a_document() {
    let mut app = crate::control::new_app();
    app.set_option(&egui::Context::default(), "language", "en").unwrap();
    assert!(app.set_option(&egui::Context::default(), "default-layout", "single").is_ok(), "the default needs no document");
    assert_eq!(app.view_defaults.display, pdfcraft_ui_egui::canvas::PageDisplay::Single);
    assert!(app.set_option(&egui::Context::default(), "cover", "on").is_err(), "cover needs an open document");
    assert!(app.set_option(&egui::Context::default(), "layout", "single").is_err(), "layout needs an open document");
}

#[test]
fn default_page_display_is_used_for_new_documents() {
    use pdfcraft_ui_egui::canvas::PageDisplay;
    // The factory default is single page, scrolling continuously, without snap jumps.
    assert_eq!(crate::control::new_app().view_defaults.display, PageDisplay::Single);
    assert!(crate::control::new_app().view_defaults.continuous);
    let mut h = harness(&[]);
    h.set_option("default-layout", "single").unwrap();
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "other.pdf", None, PAGES.to_vec()).expect("opens");
    assert_eq!(h.state().views[1].view.display, PageDisplay::Single);
    // So do documents the app creates itself (blank, combined, extracted, from images…).
    assert!(h.execute("create.blank"));
    assert_eq!(h.state().views[2].view.display, PageDisplay::Single);
    // Persisted preferences survive a restart.
    let saved = h.state().settings_snapshot();
    let mut fresh = crate::control::new_app();
    fresh.set_option(&egui::Context::default(), "language", "en").unwrap();
    fresh.apply_settings(&saved);
    assert_eq!(fresh.view_defaults.display, PageDisplay::Single);
    // Garbage keeps the previous default; casing is forgiven; legacy files lack the key.
    fresh.apply_settings(&saved_default_layout("bogus"));
    assert_eq!(fresh.view_defaults.display, PageDisplay::Single);
    fresh.apply_settings(&saved_default_layout("TWO-UP"));
    assert_eq!(fresh.view_defaults.display, PageDisplay::TwoUp);
    let mut legacy = crate::control::new_app();
    legacy.set_option(&egui::Context::default(), "language", "en").unwrap();
    legacy.apply_settings(&PdfCraftSettings::default());
    assert_eq!(legacy.view_defaults.display, PageDisplay::Single);
    assert!(legacy.view_defaults.continuous, "a legacy file without the key keeps the factory default");
}

#[test]
fn default_zoom_is_used_for_new_documents() {
    use pdfcraft_ui_egui::canvas::Fit;
    let fit_zoom = |app: &PdfCraftApp, i: usize| (app.views[i].fit(), app.views[i].zoom());
    let mut h = harness(&[]);
    // The factory default fits the width.
    assert_eq!(h.state().views[0].fit(), Fit::Width);
    h.set_option("default-zoom", "fit-page").unwrap();
    assert!(h.execute("create.blank"));
    assert_eq!(h.state().views[1].fit(), Fit::Page);
    h.set_option("default-zoom", "150%").unwrap();
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "other.pdf", None, PAGES.to_vec()).expect("opens");
    assert_eq!(fit_zoom(h.state(), 2), (Fit::None, 1.5));
    // A PDF that asks for a zoom gets it.
    let asks = String::from_utf8_lossy(PAGES).replace("/Pages 2 0 R >>", "/Pages 2 0 R /OpenAction [3 0 R /Fit] >>");
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "asks.pdf", None, asks.into_bytes()).expect("opens");
    assert_eq!(h.state().views[3].fit(), Fit::Page);
    // Typos and zooms the view can't show are refused.
    for bad in ["huge", "fit-height", "5", "9000%"] {
        assert!(h.set_option("default-zoom", bad).is_err(), "{bad}");
    }
    // Saved settings keep it, and garbage leaves it alone.
    let saved = h.state().settings_snapshot();
    let mut fresh = crate::control::new_app();
    fresh.set_option(&egui::Context::default(), "language", "en").unwrap();
    fresh.apply_settings(&saved);
    let mut garbage = PdfCraftSettings::default();
    garbage.behavior.default_zoom = Some("bogus".to_owned());
    fresh.apply_settings(&garbage);
    fresh.open_bytes(&egui::Context::default(), "again.pdf", None, PAGES.to_vec()).expect("opens");
    assert_eq!(fit_zoom(&fresh, 0), (Fit::None, 1.5));
}

#[test]
fn the_rail_zoom_field_sets_a_typed_zoom() {
    use egui::accesskit::Role;
    use egui_kittest::kittest::Queryable;
    use pdfcraft_ui_egui::canvas::Fit;
    let mut h = harness(&[]);
    let type_zoom = |h: &mut Harness<'static, PdfCraftApp>, typed: &str, key: Key| {
        h.get_by_role_and_label(Role::SpinButton, "Zoom").click();
        h.run_steps(2);
        h.key_press_modifiers(Modifiers::COMMAND, Key::A);
        h.run_steps(1);
        h.get_by_role_and_label(Role::SpinButton, "Zoom").type_text(typed);
        h.run_steps(1);
        h.key_press(key);
        h.run_steps(3);
    };
    type_zoom(&mut h, "150", Key::Enter);
    assert_eq!((h.state().views[0].fit(), h.state().views[0].zoom()), (Fit::None, 1.5));
    type_zoom(&mut h, "62.5 %", Key::Enter);
    assert_eq!(h.state().views[0].zoom(), 0.625);
    for (typed, key) in [("abc", Key::Enter), ("0", Key::Enter), ("300", Key::Escape)] {
        type_zoom(&mut h, typed, key);
        assert_eq!(h.state().views[0].zoom(), 0.625, "{typed}");
    }
    type_zoom(&mut h, "9000", Key::Enter);
    assert_eq!(h.state().views[0].zoom(), 64.0);
    h.state_mut().views[0].view.zoom = 2.0;
    h.run_steps(2);
    // The number field paints the percent unit separately from its accessible text value.
    assert_eq!(h.get_by_role_and_label(Role::SpinButton, "Zoom").value().as_deref(), Some("200"));
}

#[test]
fn always_use_my_default_zoom_and_page_layout_ignores_what_the_pdf_asks_for() {
    use pdfcraft_ui_egui::canvas::{Fit, PageDisplay};
    // Opens on page 2 at fit page, two pages side by side.
    let asks = String::from_utf8_lossy(PAGES).replace("/Pages 2 0 R >>", "/Pages 2 0 R /OpenAction [4 0 R /Fit] /PageLayout /TwoPageLeft >>");
    let open = |always: Option<&str>| {
        let mut app = crate::control::new_app();
        app.set_option(&egui::Context::default(), "language", "en").unwrap();
        if let Some(on) = always {
            app.set_option(&egui::Context::default(), "always-default-view", on).unwrap();
        }
        app.open_bytes(&egui::Context::default(), "asks.pdf", None, asks.clone().into_bytes()).expect("opens");
        let v = &app.views[0];
        (v.fit(), v.view.display, v.continuous(), v.current())
    };
    // Off by default: the document's view wins.
    assert_eq!(open(None), (Fit::Page, PageDisplay::TwoUp, false, 1));
    assert_eq!(open(Some("off")), (Fit::Page, PageDisplay::TwoUp, false, 1));
    // On: Default zoom and Default page display; the page it opens at is still the document's.
    assert_eq!(open(Some("on")), (Fit::Width, PageDisplay::Single, true, 1));
    assert!(crate::control::new_app().set_option(&egui::Context::default(), "always-default-view", "yes").is_err());
}

#[test]
fn always_use_my_default_zoom_and_page_layout_survives_a_restart() {
    let mut app = crate::control::new_app();
    assert!(!app.always_default_view, "off by default");
    app.always_default_view = true;
    let mut restarted = crate::control::new_app();
    restarted.apply_settings(&app.settings_snapshot());
    assert!(restarted.always_default_view, "the choice is kept");
    // A key that is not a boolean does not read as settings at all, so what is left of it is a file without the key.
    assert!(serde_json::from_str::<PdfCraftSettings>(r#"{"behavior":{"always_default_view":"bogus"}}"#).is_err());
    restarted.apply_settings(&PdfCraftSettings::default());
    assert!(restarted.always_default_view, "settings without the key leave it alone");
    let mut legacy = crate::control::new_app();
    legacy.apply_settings(&PdfCraftSettings::default());
    assert!(!legacy.always_default_view, "a legacy file without the key keeps it off");
}

#[test]
fn preferences_turn_on_always_use_my_default_zoom_and_page_layout() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(&[]);
    assert!(h.execute("app.preferences"));
    h.run_steps(3);
    choose_in_documents_preferences(&mut h, "Always use my default zoom and page layout");
    assert!(h.state().always_default_view);
}

#[test]
fn preferences_turn_on_always_use_my_default_zoom_and_page_layout_updates_open_documents() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(&[("continuous", "off"), ("zoom", "50")]);
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "second.pdf", None, PAGES.to_vec()).expect("opens");
    h.state_mut().views[1].view.continuous = false;
    // A fixed zoom: no fit, at 75%.
    h.state_mut().views[1].view.fit = pdfcraft_ui_egui::canvas::Fit::None;
    h.state_mut().views[1].view.zoom = 0.75;
    assert_eq!(h.state().views.len(), 2);
    assert!(!h.state().views[0].continuous());
    assert_ne!(h.state().views[0].fit(), pdfcraft_ui_egui::canvas::Fit::Width);
    assert!(!h.state().views[1].continuous());
    assert_ne!(h.state().views[1].fit(), pdfcraft_ui_egui::canvas::Fit::Width);
    assert!(h.execute("app.preferences"));
    h.run_steps(3);
    choose_in_documents_preferences(&mut h, "Always use my default zoom and page layout");
    assert!(h.state().always_default_view);
    for (i, view) in h.state().views.iter().enumerate() {
        assert!(view.continuous(), "continuous scrolling is applied to view {i}");
        assert_eq!(view.fit(), pdfcraft_ui_egui::canvas::Fit::Width, "default fit width is applied to view {i}");
    }
}

#[test]
fn preferences_toggle_continuous_scrolling_updates_active_document() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(&[("default-continuous", "off"), ("continuous", "off")]);
    assert!(!h.state().view_defaults.continuous);
    assert!(!h.state().views[0].continuous());
    assert!(h.execute("app.preferences"));
    h.run_steps(3);
    choose_in_documents_preferences(&mut h, "Continuous scrolling");
    assert!(h.state().view_defaults.continuous, "defaults are updated");
    assert!(h.state().views[0].continuous(), "active document view is updated");
}

#[test]
fn preferences_during_full_screen_updates_saved_normal_display_and_restores_on_exit() {
    use egui_kittest::kittest::Queryable;
    use pdfcraft_ui_egui::canvas::Fit;

    let mut h = harness(&[("continuous", "off"), ("zoom", "50")]);
    let v0 = &h.state().views[0];
    assert!(!v0.continuous());
    assert_eq!(v0.fit(), Fit::None);
    assert_eq!(v0.zoom(), 0.5);

    // Every view follows the window's screen mode: full screen presents it.
    h.state_mut().set_full_screen(true);
    h.run_steps(3);
    assert!(h.state().views[0].presenting());
    assert!(!h.state().views[0].continuous());
    assert_eq!(h.state().views[0].fit(), Fit::Page);

    assert!(h.execute("app.preferences"));
    h.run_steps(3);
    choose_in_documents_preferences(&mut h, "Always use my default zoom and page layout");
    assert!(h.state().always_default_view);

    {
        let v = &h.state().views[0];
        assert!(v.presenting(), "presentation display is preserved");
        assert!(!v.continuous(), "presentation display is not continuous");
        assert_eq!(v.fit(), Fit::Page, "presentation display stays fitted to page");
        assert!(v.normal_display().continuous, "saved normal display updated to continuous");
        assert_eq!(v.normal_display().fit, Fit::Width, "saved normal display updated to default fit width");
    }

    h.state_mut().set_full_screen(false);
    h.run_steps(3);
    {
        let v = &h.state().views[0];
        assert!(!v.presenting());
        assert!(v.continuous(), "continuous scrolling restored from updated preferences");
        assert_eq!(v.fit(), Fit::Width, "fit width restored from updated preferences");
    }
}

#[test]
fn preferences_toggle_continuous_during_full_screen_updates_saved_normal_display() {
    use egui_kittest::kittest::Queryable;
    use pdfcraft_ui_egui::canvas::Fit;

    let mut h = harness(&[("default-continuous", "off"), ("continuous", "off")]);
    assert!(!h.state().views[0].continuous());

    // Every view follows the window's screen mode: full screen presents it.
    h.state_mut().set_full_screen(true);
    h.run_steps(3);
    assert!(h.state().views[0].presenting());
    assert!(!h.state().views[0].continuous());
    assert_eq!(h.state().views[0].fit(), Fit::Page);

    assert!(h.execute("app.preferences"));
    h.run_steps(3);
    choose_in_documents_preferences(&mut h, "Continuous scrolling");

    assert!(h.state().view_defaults.continuous);
    {
        let v = &h.state().views[0];
        assert!(v.presenting(), "presentation display is preserved");
        assert!(!v.continuous(), "presentation display is not continuous");
        assert_eq!(v.fit(), Fit::Page, "presentation display stays fitted to page");
        assert!(v.normal_display().continuous, "saved normal display updated to continuous");
    }

    h.state_mut().set_full_screen(false);
    h.run_steps(3);
    {
        let v = &h.state().views[0];
        assert!(!v.presenting());
        assert!(v.continuous(), "continuous scrolling restored on exit");
    }
}

#[test]
fn preferences_with_always_default_view_on_toggling_continuous_preserves_custom_zoom() {
    use egui_kittest::kittest::Queryable;

    let mut h = harness(&[("always-default-view", "on"), ("default-continuous", "off"), ("continuous", "off"), ("zoom", "200")]);
    assert!(h.state().always_default_view);
    assert!(!h.state().view_defaults.continuous);
    assert!(!h.state().views[0].continuous());
    assert_eq!(h.state().views[0].zoom(), 2.0);

    assert!(h.execute("app.preferences"));
    h.run_steps(3);
    choose_in_documents_preferences(&mut h, "Continuous scrolling");

    assert!(h.state().view_defaults.continuous);
    let v = &h.state().views[0];
    assert!(v.continuous(), "continuous scrolling is updated");
    assert_eq!(v.zoom(), 2.0, "custom zoom is preserved because fit/zoom did not change");
}

#[test]
fn preferences_set_the_default_zoom() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(&[]);
    assert!(h.execute("app.preferences"));
    h.run_steps(3);
    pick_in_documents_preferences(&mut h, "Default zoom", "Zoom to page level");
    assert!(h.execute("create.blank"));
    assert_eq!(h.state().views[1].fit(), pdfcraft_ui_egui::canvas::Fit::Page);
}

#[test]
fn rail_page_display_menu_offers_acrobats_view_choices() {
    use egui_kittest::kittest::Queryable;
    use pdfcraft_ui_egui::canvas::{Fit, PageDisplay};
    let mut h = harness(&[]);
    h.run_steps(4);
    let pick = |h: &mut Harness<'static, PdfCraftApp>, item: &str| {
        h.get_by_role_and_label(egui::accesskit::Role::Button, "Page display").click();
        h.run_steps(3);
        h.get_by_label(item).click();
        h.run_steps(4);
    };
    let shown = |h: &Harness<'static, PdfCraftApp>| (h.state().views[0].view.display, h.state().views[0].continuous(), h.state().views[0].fit());
    pick(&mut h, "Fit one full page");
    assert_eq!(shown(&h), (PageDisplay::Single, false, Fit::Page), "one full page at a time, not scrolling");
    // The whole page fits at once, so the wheel turns pages without zooming out first.
    let at = h.state().views[0].viewport_rect().center();
    h.hover_at(at);
    h.run_steps(1);
    h.event(wheel(MouseWheelUnit::Line, -1.0, TouchPhase::Move));
    h.run_steps(4);
    assert_eq!(h.state().views[0].current(), 1);
    pick(&mut h, "Actual size");
    assert_eq!((h.state().views[0].fit(), h.state().views[0].zoom()), (Fit::None, 1.0));
    pick(&mut h, "Zoom to page level");
    assert_eq!(h.state().views[0].fit(), Fit::Page);
    pick(&mut h, "Fit to width scrolling");
    assert_eq!(shown(&h), (PageDisplay::Single, true, Fit::Width), "single page, scrolling continuously");
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
fn single_page_fit_page_fits_the_shown_page() {
    // Single-page view fits the page it shows: each page fills the viewport on its own.
    let mut h = mixed_harness(&[("layout", "single")]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num0); // fit page
    h.run_steps(4);
    let vp = h.state().views[0].viewport_rect();
    let r1 = rect(&h, 0).expect("page 1");
    assert!(r1.width() <= vp.width() && r1.height() <= vp.height(), "page 1 fits: {r1:?} in {vp:?}");
    let z1 = h.state().views[0].zoom();
    h.set_option("page", "2").unwrap();
    h.run_steps(4);
    let r2 = rect(&h, 1).expect("page 2");
    assert!(r2.width() <= vp.width() && r2.height() <= vp.height(), "page 2 fits: {r2:?} in {vp:?}");
    assert_ne!(h.state().views[0].zoom(), z1, "the wider page refits instead of keeping page 1's zoom");
}

#[test]
fn pages_shorter_than_the_window_sit_in_its_middle() {
    let centred = |h: &Harness<'static, PdfCraftApp>, r: egui::Rect| {
        let vp = h.state().views[0].viewport_rect();
        assert!(r.height() < vp.height(), "{r:?} is shorter than {vp:?}");
        assert!((r.center().y - vp.center().y).abs() < 1.0, "{r:?} is in the middle of {vp:?}");
    };
    let mut h = harness(&[("layout", "single"), ("continuous", "off"), ("zoom", "50")]);
    h.run_steps(4);
    centred(&h, rect(&h, 0).expect("page 1"));
    assert_eq!(h.state().views[0].current(), 0);

    // The page drawn there is the page a click finds: its link still goes to page 3.
    let r = rect(&h, 0).expect("page 1");
    let at = egui::pos2(r.min.x + r.width() * 0.5, r.min.y + r.height() * (1.0 - 325.0 / 400.0));
    h.hover_at(at);
    h.run_steps(2);
    h.drag_at(at); // press
    h.drop_at(at); // release
    h.run_steps(8);
    assert_eq!(h.state().views[0].current(), 2, "the link is where the page is drawn");

    // Scrolling continuously, the column of pages as a whole.
    let mut h = harness(&[("layout", "single"), ("continuous", "on"), ("zoom", "25")]);
    h.run_steps(4);
    let column = rect(&h, 0).expect("page 1").union(rect(&h, 2).expect("page 3"));
    centred(&h, column);

    // A page taller than the view starts at its top, as before.
    let mut h = harness(&[("layout", "single"), ("continuous", "off"), ("zoom", "250")]);
    h.run_steps(4);
    let (r, vp) = (rect(&h, 0).expect("page 1"), h.state().views[0].viewport_rect());
    assert!(r.height() > vp.height() && (r.min.y - vp.min.y).abs() < 40.0, "{r:?} in {vp:?}");
}

/// A page too tall for its margins still fits the window: it shows whole, in the middle.
#[test]
fn a_page_that_fits_without_its_margins_shows_whole() {
    let mut h = harness(&[("layout", "single"), ("continuous", "off"), ("zoom", "50")]);
    h.run_steps(4);
    let (half, vp) = (rect(&h, 0).expect("page 1"), h.state().views[0].viewport_rect());
    let spare = 20.0;
    let zoom = (50.0 * (vp.height() - spare) / half.height()).floor();
    h.set_option("zoom", &zoom.to_string()).unwrap();
    h.run_steps(4);
    let r = rect(&h, 0).expect("page 1");
    assert!(r.height() < vp.height() && r.height() + 2.0 * 28.0 > vp.height(), "{r:?} fits {vp:?} only without its margins");
    assert!(r.min.y >= vp.min.y && r.max.y <= vp.max.y, "{r:?} shows whole in {vp:?}");
    assert!((r.center().y - vp.center().y).abs() < 1.0, "{r:?} is in the middle of {vp:?}");
}

/// Two pages side by side are placed as the row they make: the taller decides, not the first.
#[test]
fn a_row_too_tall_for_the_window_starts_at_its_top_whichever_page_is_shorter() {
    let h = mixed_harness(&[("layout", "two-page"), ("zoom", "180")]);
    let vp = h.state().views[0].viewport_rect();
    let (first, second) = (rect(&h, 0).expect("page 1"), rect(&h, 1).expect("page 2"));
    assert!(first.height() > vp.height() && second.height() < vp.height(), "{first:?}, {second:?} in {vp:?}");
    let row = first.union(second);
    assert!((row.min.y - vp.min.y).abs() < 40.0, "{row:?} starts in {vp:?}");
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
    assert_eq!(h.state().views[0].current(), 2, "the link targets page 3");
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
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), name, None, pdf.to_vec()).expect("opens");
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "panel", "bookmarks").unwrap();
            app
        }
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

/// The side gutter the page view keeps clear (the spacing token); a destination's left lands past it (L-245).
fn side_gutter(h: &Harness<'static, PdfCraftApp>) -> f32 {
    h.state().views[0].spacing.as_ref().expect("laid out").side as f32
}

#[test]
fn bookmark_destinations_scroll_to_their_position() {
    use egui_kittest::kittest::Queryable;
    use pdfcraft_ui_egui::canvas::Fit;
    let mut h = dests_harness();
    // Leave enough room to scroll the destination to the start edge at 300%.
    h.set_size(egui::vec2(1300.0, 900.0));
    h.run_steps(6);
    // Fit width: the pages are taller than the window, so any point of them can reach its top.
    h.get_by_label("Section 1.1 Shapes").click();
    h.run_steps(6);
    let v = &h.state().views[0];
    assert_eq!(v.current(), 1, "the bookmark targets page 2");
    assert_eq!(v.fit(), Fit::Width, "/XYZ with zoom 0 keeps the zoom");
    let d = from_viewport_corner(&h, 1, 0.0, 0.2);
    assert!(d.y.abs() < 2.0, "y 420 on page 2 is at the top of the window, not the page top: {d:?}");

    // /XYZ left top zoom: 300%, with (125, 300), a quarter across and half way down, at the
    // window's top-left (the page is now wider than the window): its left is the edge of the
    // gutter that keeps pages clear of the quick-action bar.
    h.get_by_label("Zoomed in").click();
    h.run_steps(6);
    let v = &h.state().views[0];
    assert_eq!((v.current(), v.fit(), v.zoom()), (1, Fit::None, 3.0));
    let d = from_viewport_corner(&h, 1, 0.25, 0.5);
    let side = side_gutter(&h);
    assert!((d.x - side).abs() < 2.0 && d.y.abs() < 2.0, "the destination point is at the window's top-left, past the gutter {side}: {d:?}");

    // Unusable operands are read as null, which keeps the reader's place in user space: y 300
    // was at the window's top on page 2 (its crop box starts at y 100), so on page 3 (crop from
    // 0) y 300 is there too, a quarter down, at the current zoom.
    h.get_by_label("Broken").click();
    h.run_steps(6);
    let v = &h.state().views[0];
    assert_eq!((v.current(), v.zoom()), (2, 3.0));
    let d = from_viewport_corner(&h, 2, 0.0, 0.25);
    assert!(d.y.abs() < 2.0, "y 300 on page 3 is at the top of the window: {d:?}");
}

#[test]
fn link_destinations_scroll_to_their_position() {
    use pdfcraft_ui_egui::canvas::Fit;
    let mut h = dests_harness();
    h.set_option("zoom", "100%").unwrap();
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
    assert_eq!((v.current(), v.fit()), (1, Fit::Width), "/FitH goes to page 2 and fits its width");
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
    assert_eq!((h.state().views[0].current(), h.state().views[0].zoom()), (0, 4.0));
    let (fx, fy) = (40.0 / 300.0, 40.0 / 400.0);
    let d = from_viewport_corner(&h, 0, fx, fy);
    let side = side_gutter(&h);
    assert!((d.x - side).abs() < 2.0 && d.y.abs() < 2.0, "(40, 360) is at the window's top-left, past the gutter {side}: {d:?}");

    let r = rect(&h, 0).expect("page 1");
    let at = egui::pos2(r.min.x + r.width() * (100.0 / 300.0), r.min.y + r.height() * (1.0 - 335.0 / 400.0));
    h.hover_at(at);
    h.run_steps(2);
    h.drag_at(at); // press
    h.drop_at(at); // release
    h.run_steps(8);
    let v = &h.state().views[0];
    assert_eq!((v.current(), v.zoom()), (1, 4.0), "the link goes to page 2 at the same zoom");
    let d = from_viewport_corner(&h, 1, fx, fy);
    assert!((d.x - side).abs() < 2.0 && d.y.abs() < 2.0, "page 2 shows the same place, not its top: {d:?}");
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
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "form.pdf", None, FORM.to_vec()).expect("opens");
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app
        }
    });
    h.run_steps(8);
    h
}

#[test]
fn field_list_jumps_to_a_fields_page() {
    use egui_kittest::kittest::Queryable;
    let mut h = form_harness();
    h.set_option("panel", "fields").unwrap();
    h.run_steps(4);
    h.get_by_label("fullname");
    h.get_by_label("postcode").click();
    h.run_steps(8);
    assert_eq!(h.state().views[0].current(), 1, "postcode is on page 2");
}

#[test]
fn highlight_fields_tints_the_field_area() {
    use egui_kittest::kittest::Queryable;
    let _gpu = gpu();
    let mut h = form_harness();
    h.set_option("panel", "none").unwrap();
    // Highlighting is on by default; start from plain fields.
    h.set_option("fields", "off").unwrap();
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
    assert!(h.state().views[0].session.highlight_fields);
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

/// How many filled shapes of the frame cover the widget at `pdf` (user space, y up) on page 1 of
/// 300×400: a rectangle over its box, or a circle round its centre.
fn tints_over(h: &Harness<'static, PdfCraftApp>, pdf: [f32; 4]) -> usize {
    let r = rect(h, 0).expect("page 1 on screen");
    let to = |x: f32, y: f32| egui::pos2(r.min.x + r.width() * x / 300.0, r.min.y + r.height() * (1.0 - y / 400.0));
    let want = egui::Rect::from_two_pos(to(pdf[0], pdf[1]), to(pdf[2], pdf[3]));
    let near = |a: egui::Pos2, b: egui::Pos2| (a - b).length() < 1.0;
    h.output()
        .shapes
        .iter()
        .filter(|c| match &c.shape {
            egui::Shape::Rect(s) => s.fill.a() > 0 && near(s.rect.min, want.min) && near(s.rect.max, want.max),
            egui::Shape::Circle(s) => s.fill.a() > 0 && near(s.center, want.center()),
            _ => false,
        })
        .count()
}

/// Highlight fields marks the fields that can be filled: a read-only one (a form's locked
/// box, or one its scripts have not opened yet) is left as it is drawn, as in Acrobat.
#[test]
fn highlight_fields_leaves_read_only_fields_alone() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "locked.pdf", None, LOCKED.to_vec()).expect("opens");
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "panel", "none").unwrap();
            app.set_option(&egui::Context::default(), "fields", "on").unwrap();
            app
        }
    });
    h.run_steps(6);
    assert_eq!(tints_over(&h, [50.0, 300.0, 250.0, 330.0]), 1, "the field that can be filled is tinted");
    assert_eq!(tints_over(&h, [50.0, 200.0, 250.0, 230.0]), 0, "the read-only field is left as drawn");
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
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "radio.pdf", None, LOCKED_RADIO.to_vec()).expect("opens");
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "panel", "none").unwrap();
            app.set_option(&egui::Context::default(), "fields", "on").unwrap();
            app
        }
    });
    h.run_steps(6);
    assert_eq!(tints_over(&h, [50.0, 300.0, 90.0, 340.0]), 1, "the button that can be chosen is tinted");
    assert_eq!(tints_over(&h, [150.0, 300.0, 190.0, 340.0]), 0, "the read-only button is left as drawn");
}

/// A push button and a text field, with fields highlighted.
const BUTTON_AND_TEXT: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R] >> >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R] >> endobj
4 0 obj << /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (frame) /Rect [50 200 250 340] /P 3 0 R >> endobj
5 0 obj << /Type /Annot /Subtype /Widget /FT /Tx /T (name) /Rect [50 100 250 130] /P 3 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// A push button holds no value, so Highlight fields leaves it as drawn (an `animate`
/// animation's frames are push buttons); the text field beside it is tinted.
#[test]
fn highlight_fields_leaves_push_buttons_alone() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "button.pdf", None, BUTTON_AND_TEXT.to_vec()).expect("opens");
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "panel", "none").unwrap();
            app.set_option(&egui::Context::default(), "fields", "on").unwrap();
            app
        }
    });
    h.run_steps(6);
    assert_eq!(tints_over(&h, [50.0, 100.0, 250.0, 130.0]), 1, "the text field is tinted");
    assert_eq!(tints_over(&h, [50.0, 200.0, 250.0, 340.0]), 0, "the push button is left as drawn");
}

/// A page whose open action starts an interval writing a counter into the console.
const PAGE_TIMER: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /AA << /O << /S /JavaScript /JS (var n = 0; app.setInterval\\('n++', 10\\);) >> >> >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// The viewer runs the open action of the page being read, and its timers as they fall due.
#[test]
fn the_viewer_runs_the_pages_open_action_and_its_timers() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "timer.pdf", None, PAGE_TIMER.to_vec()).expect("opens");
            app
        }
    });
    h.run_steps(2);
    let id = h.state().views[0].id;
    assert!(h.state_mut().session.next_timer(id).is_some(), "the page's open action set the interval");
    std::thread::sleep(std::time::Duration::from_millis(30));
    h.run_steps(4);
    let n = h.state_mut().session.run_javascript(id, "n", None).expect("runs").result;
    assert!(n.as_deref().and_then(|n| n.parse::<u32>().ok()).is_some_and(|n| n >= 1), "the interval ran: {n:?}");
}

/// Under a dialog nothing acts on the document: the page's open action waits until it closes.
#[test]
fn page_actions_and_timers_wait_under_a_dialog() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "timer.pdf", None, PAGE_TIMER.to_vec()).expect("opens");
            assert!(app.execute(&egui::Context::default(), "help.shortcuts"), "the shortcuts dialog opens");
            app
        }
    });
    h.run_steps(3);
    let id = h.state().views[0].id;
    assert!(h.state_mut().session.next_timer(id).is_none(), "the open action waits under the dialog");
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(h.state_mut().session.next_timer(id).is_some(), "and runs once it closes");
}

/// Three pages: the first's open action starts an interval counting `n`; the second's open
/// action goes on to the third, and its close action stops the interval.
const PAGE_HOP: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /AA << /O << /S /JavaScript /JS (var n = 0; var t = app.setInterval\\('n++', 10\\);) >> >> >> endobj
4 0 obj << /Type /Page /Parent 2 0 R /AA << /O << /S /JavaScript /JS (this.pageNum = 2;) >> /C << /S /JavaScript /JS (app.clearInterval\\(t\\);) >> >> >> endobj
5 0 obj << /Type /Page /Parent 2 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// A page whose open action goes to another page runs no timer before its close action: the
/// interval that close action stops doesn't run in between.
#[test]
fn timers_wait_for_the_page_a_page_action_goes_to() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "hop.pdf", None, PAGE_HOP.to_vec()).expect("opens");
            app
        }
    });
    h.run_steps(2);
    let id = h.state().views[0].id;
    assert!(h.state_mut().session.next_timer(id).is_some(), "the first page's open action set the interval");
    let count = |h: &mut Harness<'static, PdfCraftApp>| h.state_mut().session.run_javascript(id, "n", None).expect("runs").result;
    let before = count(&mut h);
    std::thread::sleep(std::time::Duration::from_millis(30));
    h.state_mut().views[0].go_to_page(1);
    h.run_steps(4);
    assert_eq!(h.state().views[0].current(), 2, "the second page's open action went on");
    assert_eq!(h.state_mut().session.next_timer(id), None, "and its close action stopped the interval");
    assert_eq!(count(&mut h), before, "which didn't run in between");
}

/// A text field, with a default appearance for its editor.
const FIELD_TIMER: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] /DA (/Helv 10 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /Annots [4 0 R] >> endobj
4 0 obj << /Type /Annot /Subtype /Widget /FT /Tx /T (clock) /Rect [50 100 250 130] /P 3 0 R /F 4 >> endobj
5 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// Press field `name`'s first widget, wherever the document has it.
fn press_field(h: &mut Harness<'static, PdfCraftApp>, name: &str) {
    let p = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let f = doc.form.iter().find(|f| f.name == name).unwrap();
        pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, f, 0).expect("on screen").center()
    };
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(3);
}

/// A time-out writing the field being edited shows in its editor: leaving the field then keeps the
/// timer's value instead of writing the old one back.
#[test]
fn a_timer_writing_the_focused_field_shows_in_its_editor() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "clock.pdf", None, FIELD_TIMER.to_vec()).expect("opens");
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "zoom", "150").unwrap();
            app
        }
    });
    for _ in 0..30 {
        h.run_steps(2);
    }
    press_field(&mut h, "clock");
    assert_eq!(h.state().views[0].session.forms.focus.as_ref().map(|f| (f.name.as_str(), f.text.as_str())), Some(("clock", "")));
    // Set only now, with the field being edited: the harness's clock runs ahead of the test's.
    let id = h.state().views[0].id;
    let set = h.state_mut().session.run_javascript(id, "app.setTimeOut('getField(\"clock\").value = \"tick\";', 50);", None).expect("runs");
    assert_eq!(set.error, None);
    let value = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name == "clock").unwrap().value.clone()
    };
    for _ in 0..600 {
        if value(&h) == ["tick"] {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        h.run_steps(1);
    }
    assert_eq!(value(&h), ["tick"], "the time-out wrote the field");
    let focus = h.state().views[0].session.forms.focus.as_ref().map(|f| (f.name.clone(), f.text.clone()));
    assert_eq!(focus, Some(("clock".into(), "tick".into())), "the editor shows the time-out's value");
}

/// Two push buttons: "shown" hidden in the document and "gone" not; the page's open action shows
/// the first and hides the second. Each one's mouse-up action says it was pressed.
const SHOWN_BY_SCRIPT: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R] >> >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R] /AA << /O << /S /JavaScript /JS (var pressed = ''; getField\\('shown'\\).display = display.visible; getField\\('gone'\\).display = display.hidden;) >> >> >> endobj
4 0 obj << /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (shown) /F 6 /Rect [50 200 250 240] /P 3 0 R /AA << /U << /S /JavaScript /JS (pressed += 'shown';) >> >> >> endobj
5 0 obj << /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (gone) /F 4 /Rect [50 100 250 140] /P 3 0 R /AA << /U << /S /JavaScript /JS (pressed += 'gone';) >> >> >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// A button the page's open action showed takes presses, and one it hid takes none, whatever
/// their flags in the document say.
#[test]
fn buttons_take_presses_as_the_scripts_show_them() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "frames.pdf", None, SHOWN_BY_SCRIPT.to_vec()).expect("opens");
            app
        }
    });
    h.run_steps(2);
    let id = h.state().views[0].id;
    // Native button actions run on a worker. Frames alone do not ensure it has finished;
    // wait for the UI to accept its completion before inspecting the retained realm.
    let pressed = |h: &mut Harness<'static, PdfCraftApp>| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(12);
        loop {
            h.run_steps(1);
            let out = h.state_mut().session.run_javascript(id, "pressed", None).expect("runs");
            if out.error.as_deref() != Some("another script is active in this document") {
                assert_eq!(out.error, None, "reading the completed button's result");
                return out.result;
            }
            assert!(out.execution_refused && out.realm_untouched, "a busy probe must not change the realm");
            assert!(std::time::Instant::now() < deadline, "button script did not complete");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    press_field(&mut h, "shown");
    assert_eq!(pressed(&mut h).as_deref(), Some("shown"));
    // Do not let a still-busy first click reject the second and conceal a hit-test bug.
    press_field(&mut h, "gone");
    assert_eq!(pressed(&mut h).as_deref(), Some("shown"));
}

/// A radio group's read-only button can't be chosen by a click, while the group's other button
/// can.
#[test]
fn a_groups_read_only_button_is_not_chosen_by_a_click() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "radio.pdf", None, LOCKED_RADIO.to_vec()).expect("opens");
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "panel", "none").unwrap();
            app
        }
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
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "radios.pdf", None, RADIOS.to_vec()).expect("opens");
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "panel", "none").unwrap();
            app.set_option(&egui::Context::default(), "fields", "on").unwrap();
            app
        }
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
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "checks.pdf", None, ROUND_CHECKS.to_vec()).expect("opens");
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "panel", "none").unwrap();
            app.set_option(&egui::Context::default(), "fields", "on").unwrap();
            app
        }
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
    h.set_option("panel", "none").unwrap();
    h.run_steps(4);
    assert!(h.state().views[0].session.highlight_fields, "on by default");
    h.get_by_label("Hide field highlights").click();
    h.run_steps(4);
    assert!(!h.state().views[0].session.highlight_fields);
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "radios.pdf", None, RADIOS.to_vec()).expect("opens");
    h.run_steps(4);
    assert!(!h.state().views[1].session.highlight_fields, "the next document opens plain");
    let saved = h.state().settings_snapshot();
    let mut next = crate::control::new_app();
    next.set_option(&egui::Context::default(), "language", "en").unwrap();
    next.apply_settings(&saved);
    next.open_bytes(&egui::Context::default(), "form.pdf", None, FORM.to_vec()).expect("opens");
    assert!(!next.views[0].session.highlight_fields, "and so does the next session");
}

/// Settings saved before the default changed keep their stored choice; a missing or malformed
/// value falls back to the default instead of failing.
#[test]
fn stored_field_highlight_choice_wins_over_the_default() {
    for (stored, expected) in [
        (r#"{"behavior": {"highlight_fields": false}}"#, false),
        (r#"{"behavior": {"highlight_fields": true}}"#, true),
        (r#"{}"#, true),
        (r#"{"behavior": {"highlight_fields": "no"}}"#, true),
        (r#"{"behavior": {"highlight_fields": null}}"#, true),
    ] {
        let mut app = crate::control::new_app();
        app.set_option(&egui::Context::default(), "language", "en").unwrap();
        // A value of the wrong type does not read as settings: the app keeps its default.
        app.apply_settings(&serde_json::from_str::<PdfCraftSettings>(stored).unwrap_or_default());
        app.open_bytes(&egui::Context::default(), "form.pdf", None, FORM.to_vec()).expect("opens");
        assert_eq!(app.views[0].session.highlight_fields, expected, "{stored}");
    }
}

#[test]
fn fonts_tab_lists_fonts_and_embedding() {
    use egui_kittest::kittest::Queryable;
    let mut h = form_harness();
    h.set_option("dialog", "fonts").unwrap();
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
    h.state_mut().set_theme_preference(craft::theme::ModeChoice::Light);
    h.set_option("panel", "none").unwrap();
    h.set_option("fields", "on").unwrap();
    let props = pdfcraft_engine::FieldProps { required: Some(true), ..Default::default() };
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(&ctx_h, pdfcraft_engine::Edit::SetFieldProps { name: "fullname".into(), props: Box::new(props) });
    crate::support::settle(&mut h);
    let r = rect(&h, 0).expect("page 1");
    // Just inside the left edge of the field (x 50, y 300..330 on a 300×400 page).
    let ppp = h.ctx.pixels_per_point();
    let at = egui::pos2(r.min.x + r.width() * (50.0 / 300.0) + 0.75, r.min.y + r.height() * (1.0 - 315.0 / 400.0));
    let img = h.render().expect("renders");
    let px = *img.get_pixel((at.x * ppp) as u32, (at.y * ppp) as u32);
    let theme = craft::theme::builtins().iter().find(|theme| theme.id.as_str() == "craft.graphite").unwrap();
    let appearance = craft::theme::Appearance {
        mode: craft::theme::Mode::Light,
        contrast: craft::theme::Contrast::Standard,
        size: Default::default(),
        text_size: Default::default(),
        reduced_motion: false,
    };
    let expected = craft::theme::resolve(theme, appearance, None).tokens.color.status.bad.solid.0;
    assert_eq!(px.0, expected, "the required field uses the status-bad border");
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
        let mut h = Harness::builder().with_size(egui::vec2(1000.0, 800.0)).with_pixels_per_point(ppp).build_eframe(move |cc| {
            crate::support::install_runtime(&cc.egui_ctx);
            {
                let mut app = crate::control::new_app();
                app.set_option(&egui::Context::default(), "language", "en").unwrap();
                app.open_bytes(&egui::Context::default(), "lines.pdf", None, bytes.clone()).expect("opens");
                app.set_option(&egui::Context::default(), "left", "closed").unwrap();
                app.set_option(&egui::Context::default(), "panel", "none").unwrap();
                app.set_option(&egui::Context::default(), "zoom", "87").unwrap();
                app
            }
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
    Harness::builder().with_size(egui::vec2(1000.0, 800.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.open_bytes(&egui::Context::default(), "grid.pdf", None, pdf.clone()).expect("opens");
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "panel", "none").unwrap();
            app.set_option(&egui::Context::default(), "zoom", zoom).unwrap();
            app
        }
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
fn sidebar_collapse_and_resize_preserves_scroll_position() {
    use pdfcraft_ui_egui::canvas::Fit;
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "pages.pdf", None, PAGES.to_vec()).expect("opens");
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "panel", "bookmarks").unwrap();
            app.set_option(&egui::Context::default(), "layout", "continuous").unwrap();
            app
        }
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
    assert_eq!(v.current(), 2);
    assert_eq!(v.fit(), Fit::Width);
    let zoom_before = v.zoom();
    let rect_before = v.page_screen_rect(2).expect("page 2 on screen");
    let vp_before = v.viewport_rect();
    let rel_y_before = rect_before.min.y - vp_before.min.y;

    // Collapse bookmarks panel: available width increases, so fit-width zoom increases.
    h.set_option("panel", "none").unwrap();
    h.run_steps(6);

    let v = &h.state().views[0];
    let zoom_after = v.zoom();
    assert!(zoom_after > zoom_before, "zoom increased when panel collapsed: {zoom_after} vs {zoom_before}");
    assert_eq!(v.current(), 2, "still on page 2 after panel collapsed");
    let rect_after = v.page_screen_rect(2).expect("page 2 still on screen");
    let vp_after = v.viewport_rect();
    let rel_y_after = rect_after.min.y - vp_after.min.y;
    // Page rectangles snap to whole pixels, so a different width can move the page by one.
    assert!((rel_y_after - rel_y_before).abs() <= 1.0, "page 2 scroll position preserved: before {rel_y_before}, after {rel_y_after}");

    // Reopen bookmarks panel: available width decreases, so fit-width zoom decreases.
    h.set_option("panel", "bookmarks").unwrap();
    for _ in 0..400 {
        h.run_steps(1);
        let w = h.state().views[0].viewport_rect().width();
        if w == width {
            break;
        }
        width = w;
    }
    let v = &h.state().views[0];
    assert_eq!(v.current(), 2, "still on page 2 after panel reopened");
    let rect_reopened = v.page_screen_rect(2).expect("page 2 still on screen");
    let vp_reopened = v.viewport_rect();
    let rel_y_reopened = rect_reopened.min.y - vp_reopened.min.y;
    assert!(
        (rel_y_reopened - rel_y_before).abs() <= 1.0,
        "page 2 scroll position preserved on reopen: before {rel_y_before}, after {rel_y_reopened}"
    );
}

/// Synthetic unequal-height spreads, generated without font or image assets.
fn final_spread_at_scroll_end(heights: &[u32], cover: bool) -> (Vec<(usize, egui::Rect)>, egui::Rect, f32) {
    let kids = (0..heights.len()).map(|i| format!("{} 0 R", i + 3)).collect::<Vec<_>>().join(" ");
    let mut pdf = format!(
        "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [{kids}] /Count {} >> endobj\n",
        heights.len()
    );
    for (i, height) in heights.iter().enumerate() {
        pdf += &format!("{} 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 {height}] >> endobj\n", i + 3);
    }
    pdf += "trailer << /Root 1 0 R >>\n%%EOF";
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "unequal-spreads.pdf", None, pdf.as_bytes().to_vec()).unwrap();
            for (key, value) in [("left", "closed"), ("panel", "none"), ("layout", "two-up"), ("zoom", "200")] {
                app.set_option(&egui::Context::default(), key, value).unwrap();
            }
            app.set_option(&egui::Context::default(), "cover", if cover { "on" } else { "off" }).unwrap();
            app
        }
    });
    h.run_steps(12);
    let at = h.state().views[0].viewport_rect().center();
    h.hover_at(at);
    h.run_steps(2);
    // Reach the end through admitted wheel steps; the assertion concerns spread geometry.
    for _ in 0..8 {
        h.event(wheel(MouseWheelUnit::Line, -10.0, TouchPhase::Move));
        h.run_steps(45);
    }
    let view = &h.state().views[0];
    let last = heights.len() - 1;
    let first = if cover { last - (last + 1) % 2 } else { last - last % 2 };
    let pages = (first..=last).map(|page| (page, view.page_screen_rect(page).expect("final spread is visible"))).collect();
    let margin = view.spacing.expect("the pages were laid out").margin as f32;
    (pages, view.viewport_rect(), margin)
}

fn assert_final_spread_bottom_reachable(heights: &[u32], cover: bool) {
    let (pages, viewport, margin) = final_spread_at_scroll_end(heights, cover);
    let bottom = pages.iter().map(|(_, rect)| rect.bottom()).fold(0.0_f32, f32::max);
    assert!(
        bottom <= viewport.bottom() + 1.0,
        "final spread remains below the viewport: heights={heights:?}, cover={cover}, pages={pages:?}, viewport={viewport:?}"
    );
    // The maximum-height page reaches the bottom margin after the scroll offset settles.
    assert!((viewport.bottom() - bottom - margin).abs() < 1.0, "scroll end leaves the bottom margin: pages={pages:?}, viewport={viewport:?}");
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
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "pages.pdf", None, pdf.into_bytes()).unwrap();
            app.set_option(&egui::Context::default(), "zoom", "100").unwrap();
            app
        }
    });
    h.run_steps(8);
    h.state_mut().views[0].go_to_page(17);
    h.run_steps(8);
    assert_eq!(h.state().views[0].current(), 17);
    h.hover_at(h.state().views[0].viewport_rect().center());
    h.event(wheel(MouseWheelUnit::Point, -35.0, TouchPhase::Move));
    h.run_steps(8);
    let before = rect(&h, 17).unwrap().top() - h.state().views[0].viewport_rect().top();
    let ctx_h = h.ctx.clone();
    h.state_mut().run_command(&ctx_h, "view.read_mode");
    h.run_steps(8);
    assert_eq!(h.state().views[0].current(), 17, "switching to Read should keep the current page");
    let after = rect(&h, 17).unwrap().top() - h.state().views[0].viewport_rect().top();
    assert!((before - after).abs() < 1.0, "the position within the page moved: {before} to {after}");
    h.state_mut().views[0].go_to_page(5);
    h.run_steps(8);
    assert_eq!(h.state().views[0].current(), 5);
    h.get_by_label("All tools").click();
    h.run_steps(8);
    assert_eq!(h.state().views[0].current(), 5, "switching to All tools should keep the current page");
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "other.pdf", None, other).unwrap();
    h.set_option("zoom", "100").unwrap();
    h.run_steps(8);
    h.state_mut().views[1].go_to_page(9);
    h.run_steps(8);
    assert_eq!(h.state().views[1].current(), 9);
    h.get_by_label("pages.pdf").click();
    h.run_steps(8);
    assert_eq!(h.state().views[0].current(), 5, "the first tab keeps its own position");
    h.get_by_label("other.pdf").click();
    h.run_steps(8);
    assert_eq!(h.state().views[1].current(), 9, "the second tab keeps its own position");
}

#[test]
fn long_view_menu_fits_a_short_window() {
    use egui_kittest::kittest::Queryable;
    let mut h = Harness::builder().with_size(egui::vec2(1000.0, 400.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "pages.pdf", None, PAGES.to_vec()).unwrap();
            app
        }
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
    // The navigation rail has a Bookmarks button too; click the submenu's radio item.
    let bookmarks = h.get_by(|n| n.role() == egui::accesskit::Role::MenuItemRadio && n.label().as_deref() == Some("Bookmarks"));
    let row = bookmarks.rect();
    assert!(row.top() >= 0.0 && row.bottom() <= 400.0, "the Bookmarks submenu row is onscreen: {row:?}");
    bookmarks.click();
    h.run_steps(4);
    assert!(h.state().dock.layout.contains(&pdfcraft_ui_egui::side_panels::PanelId::Bookmarks), "the Bookmarks panel is open in the dock");
}

/// A click at `pos` with `modifiers` held, reported as the platform does: the held keys first,
/// then the press and release, then the keys let go.
fn click_with(h: &mut Harness<'static, PdfCraftApp>, pos: egui::Pos2, modifiers: Modifiers) {
    h.hover_at(pos);
    h.event(egui::Event::ModifiersChanged(modifiers));
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers });
    }
    h.run_steps(4);
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(1);
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
    assert!(h.state().full_screen());
    let v = &h.state().views[0];
    assert!(!v.continuous(), "one page at a time");
    assert_eq!(v.fit(), Fit::Page, "fitted to the screen");
    assert!(rect(&h, 0).is_some() && rect(&h, 1).is_none(), "only page 1 is on screen");
    let r = rect(&h, 0).unwrap();
    let vp = h.state().views[0].viewport_rect();
    assert!(r.height() <= vp.height() + 1.0 && r.width() <= vp.width() + 1.0, "the page fits: {r:?} in {vp:?}");
    // The Select tool's button is named by its command; V is its shortcut, not part of its name.
    assert!(h.query_all_by_label("Select").next().is_none(), "no quick tools in full screen");
    click_with(&mut h, r.center(), Modifiers::NONE);
    assert_eq!(h.state().views[0].current(), 1, "a click goes forward a page");
    h.key_press(Key::ArrowRight);
    h.run_steps(4);
    assert_eq!(h.state().views[0].current(), 2, "→ goes forward");
    h.key_press(Key::Backspace);
    h.run_steps(4);
    assert_eq!(h.state().views[0].current(), 1, "Backspace goes back");
    let r = rect(&h, 1).expect("page 2 on screen");
    click_with(&mut h, r.center(), Modifiers::SHIFT);
    assert_eq!(h.state().views[0].current(), 0, "a shift-click goes back");
    h.key_press(Key::Space);
    h.run_steps(4);
    assert_eq!(h.state().views[0].current(), 1, "Space goes forward");
    h.key_press(Key::Escape);
    h.run_steps(4);
    assert!(!h.state().full_screen(), "Escape leaves full screen");
    let v = &h.state().views[0];
    assert!(v.continuous(), "the display from before comes back");
    assert_eq!(v.fit(), Fit::None);
    assert!((v.zoom() - 0.5).abs() < 1e-3, "zoom back to 50%: {}", v.zoom());
    assert_eq!(v.current(), 1, "at the page reached in full screen");
    h.run_steps(2);
    assert!(h.query_all_by_label("Select").next().is_some(), "the quick tools are back");
}

/// #2070 review: full screen survives the file being reloaded (Esc still brings the display from
/// before back), and a document opened in full screen takes its own initial view first, so Esc
/// comes back to that and full screen stays one spread at a time.
#[test]
fn full_screen_survives_a_reload_and_keeps_a_new_documents_own_view() {
    use pdfcraft_ui_egui::canvas::{Fit, PageDisplay};
    let mut h = harness(&[("layout", "continuous"), ("zoom", "50")]);
    h.run_steps(4);
    h.key_press_modifiers(Modifiers::COMMAND, Key::L);
    h.run_steps(6);
    let ctx_h = h.ctx.clone();
    {
        let app = h.state_mut();
        let info = app.session.get(app.views[0].id).unwrap().info.clone();
        app.views[0].document_replaced(&ctx_h, &info);
    }
    h.run_steps(4);
    assert!(h.state().views[0].presenting(), "still presenting after the reload");
    h.key_press(Key::Escape);
    h.run_steps(4);
    let v = &h.state().views[0];
    assert!(v.continuous() && v.fit() == Fit::None && (v.zoom() - 0.5).abs() < 1e-3, "the display from before comes back");

    h.key_press_modifiers(Modifiers::COMMAND, Key::L);
    h.run_steps(6);
    let two_columns = String::from_utf8_lossy(PAGES).replace("/Type /Catalog /Pages 2 0 R", "/Type /Catalog /Pages 2 0 R /PageLayout /TwoColumnLeft");
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "two.pdf", None, two_columns.into_bytes()).expect("opens");
    h.run_steps(6);
    let v = h.state().views.last().unwrap();
    assert!(v.presenting() && v.view.display == PageDisplay::TwoUp && !v.continuous(), "one spread at a time");
    h.key_press(Key::Escape);
    h.run_steps(4);
    let v = h.state().views.last().unwrap();
    assert!(v.view.display == PageDisplay::TwoUp && v.continuous(), "the document's own two-column view comes back");
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
    assert!(h.state().full_screen());
    assert_eq!(h.state().quick_tool, QuickTool::Select, "Select in full screen");
    // Entering again (a second document asking for it) keeps the tool to give back.
    h.state_mut().set_full_screen(true);
    let r = rect(&h, 0).expect("page 1 on screen");
    click_with(&mut h, r.center(), Modifiers::NONE);
    assert_eq!(h.state().views[0].current(), 1, "a click goes forward");
    h.key_press(Key::Escape);
    h.run_steps(4);
    assert!(!h.state().full_screen());
    assert_eq!(h.state().quick_tool, QuickTool::Hand, "the tool from before is back");
}

/// #4415: the Pages panel's Page tools menu. Each tool acts on the selected pages (the current
/// page when none is): rotate, delete and insert a blank page here, as Pages ▸ … does.
#[test]
fn the_pages_panel_tools_act_on_the_selected_pages() {
    let mut h = harness(&[("panel", "pages")]);
    let id = h.state().views[0].id;
    let rotations = |h: &Harness<'static, PdfCraftApp>| h.state().session.get(id).unwrap().info.pages.iter().map(|p| p.rotation).collect::<Vec<_>>();
    // The panel's Page tools menu, on the right-hand side of the window: its rows are the page commands.
    let tool = |h: &mut Harness<'static, PdfCraftApp>, command: &str| {
        h.get_all_by_label("Panel Menu").find(|n| n.rect().left() > 1000.0).expect("Panel Menu in the Pages panel").click();
        h.run_steps(2);
        h.get_by_label(pdfcraft_engine::commands::command(command).expect("a page command").label).click();
        h.run_steps(3);
    };
    h.state_mut().views[0].select_pages(&[1, 2]);
    h.run_steps(2);
    tool(&mut h, "page.rotate");
    assert_eq!(rotations(&h), [0, 90, 90], "pages 2 and 3, the selection");
    tool(&mut h, "page.rotate_ccw");
    assert_eq!(rotations(&h), [0, 0, 0]);
    h.state_mut().views[0].select_pages(&[0]);
    h.run_steps(2);
    tool(&mut h, "page.delete");
    assert_eq!(h.state().session.get(id).unwrap().info.pages.len(), 2, "page 1 deleted");
    tool(&mut h, "page.insert_blank");
    assert_eq!(h.state().session.get(id).unwrap().info.pages.len(), 3);
    h.execute("edit.undo");
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
    let out = dir.join("out");
    std::fs::create_dir_all(&out).unwrap();
    // The file picker answers with the file to insert, then the folder picker with the folder to extract into.
    let files = std::sync::Arc::new(craft::host::ScriptedFiles::answering([
        craft::host::Picked::Paths(vec![more.clone()]),
        craft::host::Picked::Paths(vec![out.clone()]),
    ]));
    let host_files = files.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::app_with_files(files);
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "pages.pdf", None, PAGES.to_vec()).expect("opens");
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "panel", "pages").unwrap();
            app
        }
    });
    h.run_steps(8);
    let id = h.state().views[0].id;
    let count = |h: &Harness<'static, PdfCraftApp>| h.state().session.get(id).unwrap().info.pages.len();
    let tool = |h: &mut Harness<'static, PdfCraftApp>, command: &str| {
        h.get_all_by_label("Panel Menu").find(|n| n.rect().left() > 1000.0).expect("Panel Menu in the Pages panel").click();
        h.run_steps(2);
        h.get_by_label(pdfcraft_engine::commands::command(command).expect("a page command").label).click();
        h.run_steps(3);
    };
    assert_eq!(count(&h), 3);
    h.state_mut().views[0].select_pages(&[0]);
    tool(&mut h, "page.insert");
    assert!(crate::support::step_until(&mut h, |h| count(h) == 6), "the picked file's three pages are inserted");

    h.state_mut().views[0].select_pages(&[1, 2]);
    tool(&mut h, "page.extract");
    assert!(h.state().dialogs.is_open::<craft::ui::patterns::form_dialogs::HostedParamDialog>(), "Extract opens for the selection");
    h.get_by_label("Extract pages as separate files").click();
    h.run_steps(2);
    h.get_all_by_label("Extract").last().unwrap().click();
    // The files are written through the host's file service.
    let extracted = |out: &std::path::Path| host_files.written().iter().filter(|p| p.parent() == Some(out)).count();
    assert!(crate::support::step_until(&mut h, |_| extracted(&out) == 2), "the two selected pages, each a file");
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
        app.apply_edit(
            &egui::Context::default(),
            pdfcraft_engine::Edit::Protect(pdfcraft_engine::Protection {
                permissions_password: Some("owner".into()),
                changes: pdfcraft_engine::Changes::None,
                ..Default::default()
            }),
        );
        let bytes = app.session.save_bytes(app.views[0].id).unwrap();
        app.open_bytes(&egui::Context::default(), "restricted.pdf", None, bytes.as_ref().clone()).unwrap();
    }
    h.run_steps(6);
    let i = h.state().active.unwrap();
    let id = h.state().views[i].id;
    assert!(!h.state().session.get(id).unwrap().allows_assembly());
    // The panel's menu also holds the dock's rows, so it opens; its page tools are greyed out.
    h.get_all_by_label("Panel Menu").find(|n| n.rect().left() > 1000.0).expect("Panel Menu in the Pages panel").click();
    h.run_steps(3);
    let label = |command: &str| pdfcraft_engine::commands::command(command).expect("a page command").label;
    for command in ["page.insert_blank", "page.insert", "page.rotate_ccw", "page.rotate", "page.delete"] {
        assert!(h.get_by_label(label(command)).accesskit_node().is_disabled(), "{command} is greyed out");
    }
    h.get_by_label(label("page.delete")).click();
    h.run_steps(3);
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
    assert!(h.state().views[0].selected.items.is_empty(), "Ctrl+A now belongs to the viewer, not page thumbnails");
}

/// #4273: with the "Full screen fills the screen with one page at a time" preference off, full
/// screen keeps the page display it was entered from (continuous scrolling here), and Esc
/// brings the window's display back.
#[test]
fn full_screen_keeps_continuous_scrolling_when_fill_is_off() {
    let mut h = harness(&[("layout", "continuous"), ("zoom", "50"), ("full-screen-fill", "off")]);
    h.run_steps(4);
    h.key_press_modifiers(Modifiers::COMMAND, Key::L);
    h.run_steps(6);
    let v = &h.state().views[0];
    assert!(h.state().full_screen() && v.presenting() && !v.fills_screen());
    assert!(v.continuous(), "still scrolling continuously");
    assert!(rect(&h, 0).is_some() && rect(&h, 1).is_some(), "more than one page on screen");
    assert!((v.zoom() - 0.5).abs() < 1e-3, "the zoom stays: {}", v.zoom());
    h.key_press(Key::Escape);
    h.run_steps(4);
    assert!(!h.state().full_screen() && h.state().views[0].continuous());
}

/// #5302: Esc closes one thing at a time. In full screen with the find bar open, the first Esc
/// closes the find bar and full screen stays; the next one leaves full screen (*Adobe Acrobat
/// Help*, 2019, p. 248: in a form field, "Esc" deselects it and "Esc (press twice)" exits Full
/// Screen mode).
#[test]
fn full_screen_esc_closes_the_find_bar_before_leaving() {
    let mut h = harness(&[]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::L);
    h.run_steps(6);
    assert!(h.state().full_screen());
    h.state_mut().views[0].open_find();
    h.run_steps(2);
    assert!(h.state().views[0].find.is_some());
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(h.state().views[0].find.is_none(), "the first Esc closes the find bar");
    assert!(h.state().full_screen(), "and full screen stays");
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(!h.state().full_screen(), "the next Esc leaves full screen");
}

/// A search open in the Search panel, as the panel shows it.
fn search_in_the_panel(options: &'static [(&'static str, &'static str)]) -> Harness<'static, PdfCraftApp> {
    let mut h = harness(options);
    let ctx = h.ctx.clone();
    h.state_mut().set_option(&ctx, "panel", "search").unwrap();
    h.state_mut().views[0].open_find();
    h.state_mut().views[0].find.as_mut().expect("find opened").in_panel = true;
    h.run_steps(2);
    h.key_press_modifiers(Modifiers::COMMAND, Key::L);
    h.run_steps(6);
    assert!(h.state().full_screen());
    h
}

/// Full screen hides the Search panel, and a search shown there isn't the find bar: the first
/// Esc leaves full screen, and the panel's search is still there afterwards.
#[test]
fn full_screen_esc_leaves_and_keeps_the_search_panels_search() {
    let mut h = search_in_the_panel(&[]);
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(!h.state().full_screen(), "the first Esc leaves full screen");
    assert!(h.state().views[0].find.is_some(), "and the Search panel's search stays");
}

/// When Esc doesn't leave full screen, it doesn't close the hidden Search panel's search either.
#[test]
fn full_screen_esc_that_stays_keeps_the_search_panels_search() {
    let mut h = search_in_the_panel(&[("full-screen-escape", "off")]);
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(h.state().full_screen(), "Esc doesn't leave full screen");
    assert!(h.state().views[0].find.is_some(), "and the Search panel's search stays");
}

/// With the Fill screen preference off, full screen keeps the reading position: half way down
/// page 2, it is still half way down page 2 in full screen and again after leaving.
#[test]
fn full_screen_keeps_the_reading_position_when_fill_is_off() {
    let mut h = harness(&[("layout", "continuous"), ("zoom", "200"), ("full-screen-fill", "off")]);
    h.run_steps(4);
    h.state_mut().views[0].view.go_to_fraction(1, 0.5);
    h.run_steps(4);
    // How far page 2 starts above the top of the page view, as a share of its height.
    let scrolled_past = |h: &Harness<'static, PdfCraftApp>| {
        let page = rect(h, 1).expect("page 2 on screen");
        (h.state().views[0].viewport_rect().top() - page.top()) / page.height()
    };
    let before = scrolled_past(&h);
    assert!(before > 0.25 && before < 0.75, "page 2 is scrolled about half past the top: {before}");
    h.key_press_modifiers(Modifiers::COMMAND, Key::L);
    h.run_steps(6);
    assert!(h.state().full_screen());
    let full = scrolled_past(&h);
    assert!((full - before).abs() < 0.1, "full screen keeps the position: {full} vs {before}");
    h.key_press(Key::Escape);
    h.run_steps(6);
    assert!(!h.state().full_screen());
    let after = scrolled_past(&h);
    assert!((after - before).abs() < 0.1, "and so does leaving it: {after} vs {before}");
}

/// With the "Esc leaves full screen" preference off, Esc doesn't leave full screen and only
/// Ctrl+L (⌘L) does (*Adobe Acrobat Help*, 2019, "Full Screen preferences", p. 69: "Escape Key
/// Exits: Lets you exit Full-Screen mode by pressing the Esc key. If this option is not selected,
/// you can exit by pressing Ctrl+L").
#[test]
fn full_screen_stays_on_esc_when_esc_does_not_leave_it() {
    let mut h = harness(&[("full-screen-escape", "off")]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::L);
    h.run_steps(6);
    assert!(h.state().full_screen());
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(h.state().full_screen(), "Esc doesn't leave full screen");
    h.key_press_modifiers(Modifiers::COMMAND, Key::L);
    h.run_steps(4);
    assert!(!h.state().full_screen(), "Ctrl+L / ⌘L leaves it");
}

/// From a form field being filled, the first Esc closes the field and the second leaves full
/// screen: "Esc (press twice)" (*Adobe Acrobat Help*, 2019, "Options for working in forms",
/// p. 248).
#[test]
fn full_screen_esc_from_a_field_closes_the_field_first() {
    let mut h = harness(&[]);
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "field.pdf", None, FIELD_PAGES.to_vec()).expect("opens");
    h.run_steps(8);
    h.key_press_modifiers(Modifiers::COMMAND, Key::L);
    h.run_steps(6);
    let n = h.state().views.len() - 1;
    // Until page 1's raster is in, full screen veils it and the click reaches no field; on a slow
    // machine that takes more frames than the six above.
    for _ in 0..600 {
        if h.state().views[n].presentation_settled() {
            break;
        }
        h.run_steps(1);
    }
    assert!(h.state().views[n].presentation_settled(), "page 1 is shown");
    let r = h.state().views[n].page_screen_rect(0).expect("page 1 on screen");
    click_with(&mut h, egui::pos2(r.left() + r.width() * 0.5, r.top() + r.height() * (400.0 - 120.0) / 400.0), Modifiers::NONE);
    h.run_steps(4);
    assert!(h.ctx.egui_wants_keyboard_input(), "the field is open for typing");
    h.key_press(Key::Escape);
    h.run_steps(4);
    assert!(h.state().full_screen(), "the first Esc closes the field only");
    assert!(!h.ctx.egui_wants_keyboard_input(), "the field is closed");
    h.key_press(Key::Escape);
    h.run_steps(4);
    assert!(!h.state().full_screen(), "the second Esc leaves full screen");
}

/// Full screen shows no notice bar above the page (a form's "interactive form fields" notice, here),
/// so a page filling the screen is whole on it; the bar is back on leaving.
#[test]
fn full_screen_hides_the_notice_bar_and_the_page_fits_whole() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        let ctx = egui::Context::default();
        let mut app = crate::control::new_app();
        app.set_option(&ctx, "language", "en").unwrap();
        app.open_bytes(&ctx, "form.pdf", None, include_bytes!("../data/form.pdf").to_vec()).expect("opens");
        app.set_option(&ctx, "left", "closed").unwrap();
        app.set_option(&ctx, "panel", "none").unwrap();
        app
    });
    h.run_steps(8);
    let notice = |h: &Harness<'static, PdfCraftApp>| h.query_by_label_contains("interactive form fields").is_some();
    assert!(notice(&h), "the form notice shows in the window");
    h.key_press_modifiers(Modifiers::COMMAND, Key::L);
    h.run_steps(6);
    assert!(h.state().full_screen() && !notice(&h), "no notice bar in full screen");
    let vp = h.state().views[0].viewport_rect();
    let r = rect(&h, 0).expect("page 1 on screen");
    assert!(r.top() >= vp.top() - 1.0 && r.bottom() <= vp.bottom() + 1.0, "the page {r:?} is whole in {vp:?}");
    h.key_press(Key::Escape);
    h.run_steps(4);
    assert!(notice(&h), "the notice is back after full screen");
}

/// Delete over the Pages panel with a menu of the menu bar open is the menu's, and the Escape that
/// closes a thumbnail's context menu the menu's: no page is deleted, and the page selection stays.
/// Delete deleted the selected pages, and the panel took the Escape: it dropped the selection and
/// left the menu open. (The in-window bar takes an Escape with a menu open before the panel is
/// drawn, and a dialog takes the panel's keyboard.)
#[test]
fn delete_with_a_menu_open_and_escape_closing_a_thumbnail_menu_leave_the_pages_panel() {
    let mut h = harness(&[("panel", "pages")]);
    h.run_steps(2);
    // The first ⌘-click on another page picks it and the current page.
    let page_2 = h.get_by_label("Page 2").rect().center();
    click_with(&mut h, page_2, Modifiers::COMMAND);
    let selected = h.state().views[0].selected.clone();
    assert_eq!(selected.items.iter().copied().collect::<Vec<_>>(), [0, 1], "⌘-click picked pages 1 and 2");
    let thumb = h.get_by_label("Page 2").rect().center();
    crate::support::open_menu(&mut h, "View");
    h.hover_at(thumb);
    h.run_steps(1);
    assert!(h.query_by_role(egui::accesskit::Role::Menu).is_some(), "the menu is open");
    h.key_press(Key::Delete);
    h.run_steps(3);
    let pages = |h: &Harness<'static, PdfCraftApp>| h.state().session.get(h.state().views[0].id).unwrap().info.pages.len();
    assert_eq!(pages(&h), 3, "Delete with a menu open deleted pages");
    assert_eq!(h.state().views[0].selected, selected, "Delete with a menu open dropped the page selection");
    h.key_press(Key::Escape);
    h.run_steps(3);
    assert!(h.query_by_role(egui::accesskit::Role::Menu).is_none(), "Escape closed the menu");
    // The thumbnail's own context menu, the panel keeping the keyboard: Escape closes the menu.
    h.hover_at(thumb);
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: thumb, button: egui::PointerButton::Secondary, pressed, modifiers: Modifiers::NONE });
        h.run_steps(1);
    }
    h.run_steps(2);
    assert!(h.query_by_role(egui::accesskit::Role::Menu).is_some(), "the thumbnail's menu is open");
    assert_eq!(h.state().views[0].selected, selected, "the right-click kept the page selection");
    h.key_press(Key::Escape);
    h.run_steps(3);
    assert!(h.query_by_role(egui::accesskit::Role::Menu).is_none(), "Escape closed the thumbnail's menu");
    assert_eq!(h.state().views[0].selected, selected, "the Escape that closed the thumbnail's menu dropped the page selection");
    assert_eq!(pages(&h), 3);
}

/// Full screen entered over the OCR Verify screen draws the page alone, and its keys are the
/// pages': Escape leaves it. The hidden screen still counted as over the pages, so only ⌘L did.
#[test]
fn escape_leaves_full_screen_entered_over_the_ocr_verify_screen() {
    let mut h = harness(&[]);
    h.state_mut().ocr_verify.open = true;
    h.run_steps(3);
    h.key_press_modifiers(Modifiers::COMMAND, Key::L);
    h.run_steps(4);
    assert!(h.state().full_screen(), "⌘L entered full screen");
    h.key_press(Key::Escape);
    h.run_steps(4);
    assert!(!h.state().full_screen(), "Escape left full screen");
    assert!(h.state().ocr_verify.open, "the OCR Verify screen is back");
}

/// Three 300×400 pt pages; the second comes in with a one-second Wipe and stays one second
/// (`/Trans`, `/Dur`), the others have neither.
const PRESENTATION: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R >> endobj
4 0 obj << /Type /Page /Parent 2 0 R /Trans << /S /Wipe /D 1 >> /Dur 1 >> endobj
5 0 obj << /Type /Page /Parent 2 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// The presentation document in full screen, frames a tenth of a second apart, every raster in.
fn presenting(options: &'static [(&'static str, &'static str)]) -> Harness<'static, PdfCraftApp> {
    presenting_pdf(PRESENTATION, options)
}

/// [`presenting`] with another document.
fn presenting_pdf(pdf: &'static [u8], options: &'static [(&'static str, &'static str)]) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(0.1).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "slides.pdf", None, pdf.to_vec()).expect("opens");
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "panel", "none").unwrap();
            app.set_option(&egui::Context::default(), "animation_effects", "on").unwrap();
            for (k, v) in options {
                app.set_option(&egui::Context::default(), k, v).unwrap();
            }
            app
        }
    });
    h.run_steps(4);
    h.key_press_modifiers(Modifiers::COMMAND, Key::L);
    h.run_steps(4);
    assert!(h.state().full_screen());
    settle(&mut h);
    h
}

/// Run frames until the renderer has nothing left to deliver (bounded, in real time: the
/// renderer's threads do not follow the harness clock).
fn settle(h: &mut Harness<'static, PdfCraftApp>) {
    for _ in 0..300 {
        h.run_steps(1);
        if !h.state().render_pending() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Run frames until `done` holds (bounded), returning the number of frames it took.
fn frames_until(h: &mut Harness<'static, PdfCraftApp>, done: impl Fn(&PdfCraftApp) -> bool) -> usize {
    for n in 0..300 {
        if done(h.state()) {
            return n;
        }
        h.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("never happened");
}

/// #379: in full screen, a page's /Trans plays when it comes into view (from the page before,
/// once the new one has rendered), and its /Dur turns to the next page once it is up; the last
/// page, with no display time, stays.
#[test]
fn full_screen_plays_a_page_transition_and_honours_the_display_time() {
    let mut h = presenting(&[]);
    assert!(!h.state().views[0].presenting_a_transition(), "page 1 just shows");
    h.key_press(Key::ArrowRight);
    h.run_steps(1);
    assert_eq!(h.state().views[0].current(), 1);
    assert!(h.state().views[0].presenting_a_transition(), "page 2 comes in with its Wipe");
    // The Wipe is painted, not only timed: once page 2 has rendered, a frame shows a strip of it as
    // tall as the page and only part of its width.
    let wipe_strip = |h: &Harness<'static, PdfCraftApp>| {
        h.state().views[0].page_screen_rect(1).is_some_and(|page| {
            h.output().shapes.iter().any(|clipped| {
                let egui::Shape::Mesh(mesh) = &clipped.shape else { return false };
                let b = mesh.calc_bounds();
                mesh.texture_id != egui::TextureId::default()
                    && (b.min.x - page.min.x).abs() < 1.0
                    && (b.min.y - page.min.y).abs() < 1.0
                    && (b.max.y - page.max.y).abs() < 1.0
                    && b.width() > 1.0
                    && b.width() < page.width() - 1.0
            })
        })
    };
    let mut steps = 0;
    while !wipe_strip(&h) && steps < 300 {
        h.run_steps(1);
        steps += 1;
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(wipe_strip(&h), "the Wipe paints part of page 2 ({steps} frames)");
    // From the first painted strip, when page 2's raster is in: time waiting for the raster does
    // not count. A harness step can process multiple queued events and advance more than one frame.
    let first_strip = h.ctx.input(|i| i.time);
    frames_until(&mut h, |a| !a.views[0].presenting_a_transition());
    let elapsed = h.ctx.input(|i| i.time) - first_strip;
    assert!(elapsed >= 0.8, "the Wipe takes its second ({elapsed} s after the first strip)");
    assert_eq!(h.state().views[0].current(), 1, "still page 2 when the transition ends");
    h.run_steps(5);
    assert_eq!(h.state().views[0].current(), 1, "half of its display time");
    h.run_steps(7);
    assert_eq!(h.state().views[0].current(), 2, "its one-second display time is up");
    assert!(!h.state().views[0].presenting_a_transition(), "page 3 has no transition");
    h.run_steps(30);
    assert_eq!(h.state().views[0].current(), 2, "the last page stays");
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(!h.state().views[0].presenting_a_transition());
}

/// In full screen the pointer hides after three still seconds, and shows again when it moves.
#[test]
fn full_screen_hides_a_still_pointer() {
    let mut h = presenting(&[]);
    let centre = h.state().views[0].viewport_rect().center();
    h.hover_at(centre);
    h.run_steps(2);
    assert_ne!(h.output().platform_output.cursor_icon, egui::CursorIcon::None, "just moved");
    h.run_steps(32);
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::None, "still for over three seconds");
    h.hover_at(centre + egui::vec2(40.0, 0.0));
    h.run_steps(1);
    assert_ne!(h.output().platform_output.cursor_icon, egui::CursorIcon::None, "moved again");
}

/// A document replaced while full screen plays a transition (a reload) starts afresh: none of the
/// old document's transition, page or timer carries over.
#[test]
fn full_screen_starts_afresh_when_the_document_is_replaced() {
    let mut h = presenting(&[]);
    h.key_press(Key::ArrowRight);
    h.run_steps(1);
    assert!(h.state().views[0].presenting_a_transition(), "page 2 comes in with its Wipe");
    let ctx_h = h.ctx.clone();
    {
        let app = h.state_mut();
        let info = app.session.get(app.views[0].id).unwrap().info.clone();
        app.views[0].document_replaced(&ctx_h, &info);
    }
    assert!(!h.state().views[0].presenting_a_transition(), "the old transition is dropped");
}

/// A still pointer hides over a link too: the page's own cursor (the hand over a link) is chosen
/// first, and full screen hides the pointer after it.
#[test]
fn full_screen_hides_a_still_pointer_over_a_link() {
    let mut h = harness(&[]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::L);
    h.run_steps(6);
    assert!(h.state().full_screen());
    // Page 1's link covers x 50–250, y 300–350 of the 300×400 pt page (from its bottom).
    let r = rect(&h, 0).expect("page 1 on screen");
    let on_link = egui::pos2(r.left() + r.width() * 150.0 / 300.0, r.top() + r.height() * 75.0 / 400.0);
    h.hover_at(on_link);
    h.run_steps(1);
    assert_ne!(h.output().platform_output.cursor_icon, egui::CursorIcon::None, "just moved, over the link");
    h.run_steps(16);
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::None, "still for over three seconds");
}

/// A text field on page 1 of three.
const FIELD_PAGES: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R] >> >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /Annots [6 0 R] >> endobj
4 0 obj << /Type /Page /Parent 2 0 R >> endobj
5 0 obj << /Type /Page /Parent 2 0 R >> endobj
6 0 obj << /Type /Annot /Subtype /Widget /FT /Tx /T (name) /Rect [50 100 250 140] /P 3 0 R /F 4 >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// Full screen keeps the pointer active "so that you can click links and open notes" (*Adobe
/// Acrobat Help*, 2019, p. 65): a click on a form field opens it for typing rather than turning
/// the page, and a click beside it still turns.
#[test]
fn full_screen_click_on_a_field_fills_it_and_turns_no_page() {
    let mut h = harness(&[]);
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "field.pdf", None, FIELD_PAGES.to_vec()).expect("opens");
    h.run_steps(8);
    h.key_press_modifiers(Modifiers::COMMAND, Key::L);
    h.run_steps(6);
    let n = h.state().views.len() - 1;
    // Until page 1's raster is in, full screen veils it and a click turns the page; on a slow
    // machine that takes more frames than the six above.
    for _ in 0..600 {
        if h.state().views[n].presentation_settled() {
            break;
        }
        h.run_steps(1);
    }
    assert!(h.state().views[n].presentation_settled(), "page 1 is shown");
    let r = h.state().views[n].page_screen_rect(0).expect("page 1 on screen");
    let on_field = egui::pos2(r.left() + r.width() * 0.5, r.top() + r.height() * (400.0 - 120.0) / 400.0);
    click_with(&mut h, on_field, Modifiers::NONE);
    h.run_steps(8);
    assert_eq!(h.state().views[n].current(), 0, "no page turned");
    assert!(h.ctx.egui_wants_keyboard_input(), "the field is open for typing");
    click_with(&mut h, egui::pos2(r.center().x, r.top() + 20.0), Modifiers::NONE);
    h.run_steps(4);
    assert_eq!(h.state().views[n].current(), 1, "a click beside the field turns the page");
}

/// Shift+Ctrl+Page Down or Down Arrow goes to the last page, and Shift+Ctrl+Page Up or Up Arrow
/// to the first, as Home and End do (*Adobe Acrobat Help*, 2019, "Keys for navigating a PDF",
/// p. 83), in full screen and out of it.
#[test]
fn shift_ctrl_page_keys_and_arrows_go_to_the_first_and_last_page() {
    for full_screen in [false, true] {
        let mut h = harness(&[("layout", "single")]);
        if full_screen {
            h.key_press_modifiers(Modifiers::COMMAND, Key::L);
            h.run_steps(6);
        }
        let both = Modifiers::COMMAND | Modifiers::SHIFT;
        for (k, want) in [(Key::PageDown, 2), (Key::PageUp, 0), (Key::ArrowDown, 2), (Key::ArrowUp, 0)] {
            h.key_press_modifiers(both, k);
            h.run_steps(6);
            assert_eq!(h.state().views[0].current(), want, "{k:?}, full screen {full_screen}");
        }
    }
}

/// Two pages, the second with a text field and a five-second Wipe.
const FIELD_BEHIND_TRANSITION: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] >> >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R >> endobj
4 0 obj << /Type /Page /Parent 2 0 R /Annots [5 0 R] /Trans << /S /Wipe /D 5 >> >> endobj
5 0 obj << /Type /Annot /Subtype /Widget /FT /Tx /T (name) /Rect [50 100 250 140] /P 4 0 R /F 4 >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// #5316 review: while full screen turns to a page (its transition playing, or its raster on the
/// way), the page isn't the one on screen yet, so a click there opens none of its fields; once
/// the page is in, the same click does.
#[test]
fn full_screen_takes_no_page_input_until_the_page_is_on_screen() {
    let mut h = presenting_pdf(FIELD_BEHIND_TRANSITION, &[]);
    h.key_press(Key::ArrowRight);
    h.run_steps(1);
    assert_eq!(h.state().views[0].current(), 1);
    assert!(h.state().views[0].presenting_a_transition(), "page 2 comes in with its Wipe");
    let r = h.state().views[0].page_screen_rect(1).expect("page 2 on screen");
    let on_field = egui::pos2(r.left() + r.width() * 0.5, r.top() + r.height() * (400.0 - 120.0) / 400.0);
    click_with(&mut h, on_field, Modifiers::NONE);
    h.run_steps(2);
    assert!(h.state().views[0].presenting_a_transition(), "the Wipe is still playing");
    assert!(h.state().views[0].session.forms.focus.is_none(), "the field behind the transition isn't opened");
    frames_until(&mut h, |a| !a.views[0].presenting_a_transition());
    h.run_steps(1);
    click_with(&mut h, on_field, Modifiers::NONE);
    h.run_steps(4);
    assert_eq!(h.state().views[0].session.forms.focus.as_ref().map(|f| f.name.as_str()), Some("name"), "once page 2 is in, the field opens");
}

/// [`FIELD_BEHIND_TRANSITION`] with a Wipe of 0.3 s, shorter than a click may be held.
const FIELD_BEHIND_SHORT_TRANSITION: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] >> >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R >> endobj
4 0 obj << /Type /Page /Parent 2 0 R /Annots [5 0 R] /Trans << /S /Wipe /D 0.3 >> >> endobj
5 0 obj << /Type /Annot /Subtype /Widget /FT /Tx /T (name) /Rect [50 100 250 140] /P 4 0 R /F 4 >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// #5316 review: a press made while full screen turns to a page belongs to nothing on it, through
/// its release: let go on the field just after the transition ends, within a click's time, it
/// still opens nothing.
#[test]
fn full_screen_press_made_during_a_transition_opens_nothing_when_released() {
    let mut h = presenting_pdf(FIELD_BEHIND_SHORT_TRANSITION, &[]);
    let r = h.state().views[0].page_screen_rect(0).expect("page 1 on screen");
    let on_field = egui::pos2(r.left() + r.width() * 0.5, r.top() + r.height() * (400.0 - 120.0) / 400.0);
    h.hover_at(on_field);
    h.run_steps(1);
    // Page 2 renders ahead: once nothing is left to render, its Wipe starts with the key.
    settle(&mut h);
    h.key_press(Key::ArrowRight);
    h.run_steps(1);
    assert!(h.state().views[0].presenting_a_transition(), "page 2 comes in with its Wipe");
    h.event(egui::Event::PointerButton { pos: on_field, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(1);
    assert!(h.state().views[0].presenting_a_transition(), "pressed while the Wipe plays");
    let frames = frames_until(&mut h, |a| !a.views[0].presenting_a_transition());
    assert!(frames <= 4, "the Wipe ends within a click's time ({frames} frames of 0.1 s)");
    h.event(egui::Event::PointerButton { pos: on_field, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(4);
    assert!(h.state().views[0].session.forms.focus.is_none(), "released on the field once page 2 is in: nothing opens");
}

/// In full screen a double-click on a note opens its pop-up, and turns no page: the pointer stays
/// active "so that you can click links and open notes" (*Adobe Acrobat Help*, 2019, p. 65).
#[test]
fn full_screen_double_click_on_a_note_opens_it_and_turns_no_page() {
    const NOTE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /Annots [4 0 R] >> endobj
4 0 obj << /Type /Annot /Subtype /Text /Rect [100 100 124 124] /Contents (Hello) /T (A) /F 4 >> endobj
5 0 obj << /Type /Page /Parent 2 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF";
    let mut h = harness_stepping(1.0 / 60.0, &[]);
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "note.pdf", None, NOTE.to_vec()).expect("opens");
    h.run_steps(8);
    h.key_press_modifiers(Modifiers::COMMAND, Key::L);
    h.run_steps(6);
    // Until the page's raster is in: before it, full screen takes no press on the page.
    settle(&mut h);
    let n = h.state().views.len() - 1;
    let r = h.state().views[n].page_screen_rect(0).expect("page 1 on screen");
    let p = egui::pos2(r.left() + r.width() * 112.0 / 300.0, r.top() + r.height() * (400.0 - 112.0) / 400.0);
    h.hover_at(p);
    h.run_steps(2);
    for _ in 0..2 {
        for pressed in [true, false] {
            h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        }
        h.step();
    }
    // Past the double-click time, when a turn would have come.
    h.run_steps(30);
    assert_eq!(h.state().views[n].current(), 0, "no page turned");
    // A double-click on a note opens its pop-up window, as in Acrobat (comment.popups).
    let note =
        h.state().session.get(h.state().views[n].id).unwrap().info.annotations.iter().find(|a| a.subtype == "Text").cloned().expect("the note");
    assert_eq!(note.popup.map(|w| w.open), Some(true), "the note's pop-up is open");
}

/// Escape leaves full screen while a transition plays and while a display time counts, and the
/// display time stops with it: the page reached stays.
#[test]
fn escape_leaves_full_screen_mid_transition_and_stops_the_display_time() {
    for mid_transition in [true, false] {
        let mut h = presenting(&[]);
        h.key_press(Key::ArrowRight);
        h.run_steps(1);
        assert!(h.state().views[0].presenting_a_transition(), "page 2 comes in with its Wipe");
        if !mid_transition {
            frames_until(&mut h, |a| !a.views[0].presenting_a_transition());
            h.run_steps(3);
            assert_eq!(h.state().views[0].current(), 1, "page 2's display time is counting");
        }
        h.key_press(Key::Escape);
        h.run_steps(1);
        assert!(!h.state().full_screen(), "Escape leaves full screen (mid-transition: {mid_transition})");
        assert!(!h.state().views[0].presenting_a_transition());
        h.run_steps(30);
        assert_eq!(h.state().views[0].current(), 1, "no page turns by itself after leaving (mid-transition: {mid_transition})");
    }
}

/// Every way full screen moves: forward (→, Space, Enter, Page Down, a click), back (←, Backspace,
/// Shift+Enter, Page Up, a right-click, a Shift-click), first (Home, Shift+Ctrl+Page Up), last (End,
/// Shift+Ctrl+Page Down) and out (Escape, Ctrl+L).
#[test]
fn full_screen_every_key_and_click_goes_where_it_says() {
    #[derive(Clone, Copy, Debug)]
    enum Input {
        Key(Key, Modifiers),
        Click(egui::PointerButton, Modifiers),
    }
    use Input::{Click, Key as K};
    use egui::PointerButton::{Primary, Secondary};
    let both = Modifiers::COMMAND | Modifiers::SHIFT;
    // (input, page before, page after); a page past the end stays where it is.
    let moves = [
        (K(Key::ArrowRight, Modifiers::NONE), 0, 1),
        (K(Key::Space, Modifiers::NONE), 0, 1),
        (K(Key::Enter, Modifiers::NONE), 0, 1),
        (K(Key::PageDown, Modifiers::NONE), 0, 1),
        (Click(Primary, Modifiers::NONE), 0, 1),
        (K(Key::ArrowLeft, Modifiers::NONE), 1, 0),
        (K(Key::Backspace, Modifiers::NONE), 1, 0),
        (K(Key::Enter, Modifiers::SHIFT), 1, 0),
        (K(Key::PageUp, Modifiers::NONE), 1, 0),
        (Click(Secondary, Modifiers::NONE), 1, 0),
        (Click(Primary, Modifiers::SHIFT), 1, 0),
        (K(Key::Home, Modifiers::NONE), 2, 0),
        (K(Key::PageUp, both), 2, 0),
        (K(Key::End, Modifiers::NONE), 0, 2),
        (K(Key::PageDown, both), 0, 2),
    ];
    // A document with no display times, so no page turns by itself meanwhile.
    let mut h = harness(&[]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::L);
    h.run_steps(6);
    assert!(h.state().full_screen());
    for (input, from, to) in moves {
        h.state_mut().views[0].go_to_page(from);
        h.run_steps(4);
        assert_eq!(h.state().views[0].current(), from, "{input:?}: on page {}", from + 1);
        match input {
            K(key, modifiers) => {
                h.key_press_modifiers(modifiers, key);
                h.run_steps(4);
            }
            Click(button, modifiers) => {
                // Clear of page 1's link, which a click would follow instead.
                let r = rect(&h, from).expect("the page is on screen");
                let at = egui::pos2(r.center().x, r.bottom() - 20.0);
                // The modifiers are held before the click, as a platform reports them.
                h.event(egui::Event::ModifiersChanged(modifiers));
                h.hover_at(at);
                h.run_steps(1);
                for pressed in [true, false] {
                    h.event(egui::Event::PointerButton { pos: at, button, pressed, modifiers });
                }
                h.run_steps(6);
                h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
                if button == Secondary {
                    assert!(h.query_by_role(egui::accesskit::Role::Menu).is_none(), "a full-screen right-click does not open a page menu");
                }
            }
        }
        assert_eq!(h.state().views[0].current(), to, "{input:?} from page {}", from + 1);
        assert!(h.state().full_screen(), "{input:?} stays in full screen");
    }
    for (key, modifiers) in [(Key::Escape, Modifiers::NONE), (Key::L, Modifiers::COMMAND)] {
        if !h.state().full_screen() {
            h.key_press_modifiers(Modifiers::COMMAND, Key::L);
            h.run_steps(6);
        }
        assert!(h.state().full_screen());
        h.key_press_modifiers(modifiers, key);
        h.run_steps(4);
        assert!(!h.state().full_screen(), "{key:?} leaves full screen");
    }
}

/// A one-page PDF whose content inflates past the renderer's per-stream limit (#623): a window
/// of whitespace the interpreter can never complete, so the page is cut and says so.
fn cut_content_pdf() -> Vec<u8> {
    use std::io::Write as _;
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    let chunk = vec![0u8; 1 << 20];
    for _ in 0..288 {
        z.write_all(&chunk).unwrap();
    }
    let stream = z.finish().unwrap();
    let mut pdf = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] /Contents 4 0 R >> endobj\n".to_vec();
    pdf.extend_from_slice(format!("4 0 obj << /Filter /FlateDecode /Length {} >> stream\n", stream.len()).as_bytes());
    pdf.extend_from_slice(&stream);
    pdf.extend_from_slice(b"\nendstream endobj\ntrailer << /Root 1 0 R >>\n%%EOF\n");
    pdf
}

/// A page whose content was cut at the renderer's limits is named in a notice, and listed for
/// automation (`ui.state`, by page number), instead of looking complete (#623).
#[test]
fn a_page_whose_content_was_cut_is_named_in_a_notice() {
    let _gpu = gpu();
    let pdf = cut_content_pdf();
    let (client, channel) = std::sync::mpsc::channel();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.attach_control(craft::automation::ControlChannel::new(channel), None);
            app.open_bytes(&egui::Context::default(), "cut.pdf", None, pdf).expect("opens");
            app
        }
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while h.state().views[0].cut_pages().is_empty() {
        assert!(std::time::Instant::now() < deadline, "the cut page was never reported; errors={:?}", h.state().views[0].page_errors());
        h.run_steps(2);
    }
    h.run_steps(2);
    assert_eq!(h.state().views[0].cut_pages(), [0]);
    assert!(h.state().views[0].page_errors().is_empty(), "a cut page still draws");
    h.get_by_label_contains("Page 1 holds more content than PdfCraft can read");
    // Automation counts pages from 1, as the notice does (review on #2113).
    let state = crate::control::ok(&mut h, &client, "ui.state", serde_json::json!({}));
    assert_eq!(state["active"]["cut_pages"], serde_json::json!([1]), "{state}");
}
