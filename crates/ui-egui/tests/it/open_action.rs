//! A document opens where its `/OpenAction` says (ISO 32000-2 §12.3.2.2 Table 149, §12.3.2.4):
//! the page, and the place on it, of an explicit destination, a go-to action's or a named one;
//! and a full-screen document's `/NonFullScreenPageMode` panel (Table 147).

use egui::{Key, Modifiers};
use egui_kittest::Harness;
use pdfcraft_render::DestView;
use pdfcraft_ui_egui::canvas::Fit;
use pdfcraft_ui_egui::{PdfCraftApp, RightPanel};

/// Six Letter (612×792) pages, objects 3 to 8; `catalog` is added to the catalog dictionary.
/// Object 9 is an outline (one bookmark, object 10) that `/Outlines 9 0 R` can use.
fn fixture(catalog: &str) -> Vec<u8> {
    let n = 6;
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 3 + i)).collect();
    let mut objs = vec![format!("<< /Type /Catalog /Pages 2 0 R {catalog} >>"), format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" "))];
    for _ in 0..n {
        objs.push("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_string());
    }
    objs.push("<< /Type /Outlines /First 10 0 R /Last 10 0 R /Count 1 >>".to_string());
    objs.push("<< /Title (Chapter) /Parent 9 0 R /Dest [6 0 R /Fit] >>".to_string());
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn harness(catalog: &str) -> Harness<'static, PdfCraftApp> {
    let bytes = fixture(catalog);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.set_option("left", "closed").unwrap();
        app.open_bytes("open.pdf", None, bytes).expect("opens");
        app
    });
    h.run_steps(4);
    settle(&mut h);
    h
}

/// Run frames until rendering has finished and the layout holds still.
fn settle(h: &mut Harness<'static, PdfCraftApp>) {
    for _ in 0..200 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// The page at the top of the window and the user-space height there: where a bookmark made now
/// (⌘B) goes.
fn window_top(h: &mut Harness<'static, PdfCraftApp>) -> (usize, f32) {
    h.key_press_modifiers(Modifiers::COMMAND, Key::B);
    h.run_steps(3);
    let id = h.state().views[0].id;
    let outline = &h.state().session.get(id).unwrap().info.outline;
    let made = outline.iter().find(|b| b.title == "Untitled").expect("the new bookmark");
    match made.view {
        DestView::Xyz { top: Some(top), .. } => (made.page.expect("a page"), top),
        other => panic!("expected /XYZ with a top, got {other:?}"),
    }
}

/// The left edge of `page` on screen and its scale (screen points per user-space point).
fn page_left(h: &Harness<'static, PdfCraftApp>, page: usize) -> (f32, f32) {
    let rects = h.state().views[0].visible_page_rects();
    let (_, r) = rects.iter().find(|(p, _)| *p == page).expect("the page is shown");
    (r.left(), r.width() / 612.0)
}

fn near(got: f32, want: f32, what: &str) {
    assert!((got - want).abs() < 2.0, "{what}: {got} vs {want}");
}

#[test]
fn an_xyz_open_action_puts_its_point_at_the_window_top_left() {
    let mut h = harness("/OpenAction [5 0 R /XYZ 100 500 2]");
    assert_eq!(h.state().views[0].current, 2);
    assert_eq!((h.state().views[0].fit, h.state().views[0].zoom), (Fit::None, 2.0));
    // `left`: the page sits 100 user points further left than one opened at left 0.
    let (left, scale) = page_left(&h, 2);
    let (left0, _) = page_left(&harness("/OpenAction [5 0 R /XYZ 0 500 2]"), 2);
    near(left0 - left, 100.0 * scale, "the page moved left by `left`");
    let (page, top) = window_top(&mut h);
    assert_eq!(page, 2);
    near(top, 500.0, "y 500 is at the window's top edge");
}

#[test]
fn a_fith_open_action_puts_its_top_at_the_window_top() {
    let mut h = harness("/OpenAction [4 0 R /FitH 400]");
    assert_eq!(h.state().views[0].current, 1);
    assert_eq!(h.state().views[0].fit, Fit::Width);
    let (page, top) = window_top(&mut h);
    assert_eq!(page, 1);
    near(top, 400.0, "y 400 is at the window's top edge");
}

#[test]
fn a_named_open_destination_opens_its_page() {
    for catalog in [
        // PDF 1.1: a name, through the catalog's /Dests.
        "/OpenAction << /S /GoTo /D /there >> /Dests << /there [7 0 R /XYZ 0 792 0] >>",
        // A string, through the /Names /Dests name tree.
        "/OpenAction << /S /GoTo /D (there) >> /Names << /Dests << /Names [(there) [7 0 R /XYZ 0 792 0]] >> >>",
        // The destination given as a name, without an action.
        "/OpenAction /there /Dests << /there [7 0 R /XYZ 0 792 0] >>",
    ] {
        let mut h = harness(catalog);
        assert_eq!(h.state().views[0].current, 4, "{catalog}");
        // A zoom of 0 keeps the default zoom.
        assert_eq!(h.state().views[0].fit, Fit::Width, "{catalog}");
        let (page, top) = window_top(&mut h);
        assert_eq!(page, 4, "{catalog}");
        near(top, 792.0, "the page's top is at the window's top edge");
    }
    // A name nothing defines opens on the first page.
    let h = harness("/OpenAction << /S /GoTo /D /missing >> /Dests << /there [7 0 R /Fit] >>");
    assert_eq!(h.state().views[0].current, 0);
}

#[test]
fn always_my_default_view_keeps_the_zoom_and_goes_to_the_page_top() {
    let bytes = fixture("/OpenAction [5 0 R /XYZ 100 500 2]");
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.set_option("left", "closed").unwrap();
        app.always_default_view = true;
        app.open_bytes("open.pdf", None, bytes).expect("opens");
        app
    });
    h.run_steps(4);
    settle(&mut h);
    assert_eq!(h.state().views[0].current, 2);
    assert_eq!(h.state().views[0].fit, Fit::Width);
    let (page, top) = window_top(&mut h);
    assert_eq!(page, 2);
    near(top, 792.0, "the page's top");
}

#[test]
fn a_full_screen_document_shows_its_non_full_screen_panel_outside_full_screen() {
    for (mode, want) in [("UseThumbs", Some(RightPanel::Pages)), ("UseOC", Some(RightPanel::Layers)), ("UseNone", None)] {
        let mut h = harness(&format!("/PageMode /FullScreen /ViewerPreferences << /NonFullScreenPageMode /{mode} >>"));
        assert!(h.state().pending_full_screen.is_some(), "{mode}: full screen is asked for");
        assert_eq!(h.state().right, want, "{mode}: the panel behind the prompt");
        // Declining it leaves that panel.
        let ctx = h.ctx.clone();
        h.state_mut().resolve_full_screen_prompt(&ctx, false);
        h.run_steps(2);
        assert_eq!((h.state().full_screen, h.state().right), (false, want), "{mode}: declined");
        // Entering and leaving full screen comes back to it.
        h.state_mut().apply_full_screen(true);
        h.run_steps(2);
        h.state_mut().apply_full_screen(false);
        h.run_steps(2);
        assert_eq!(h.state().right, want, "{mode}: after full screen");
    }
    // Without /PageMode /FullScreen the entry means nothing.
    let h = harness("/ViewerPreferences << /NonFullScreenPageMode /UseThumbs >>");
    assert_eq!(h.state().right, None);
}

#[test]
fn a_full_screen_document_with_an_outline_shows_it_only_when_it_asks_to() {
    // UseNone (and its absence, the default) shows no outline outside full screen; UseOutlines does.
    for (prefs, want) in [
        ("", None),
        ("/ViewerPreferences << /NonFullScreenPageMode /UseNone >>", None),
        ("/ViewerPreferences << /NonFullScreenPageMode /UseOutlines >>", Some(RightPanel::Bookmarks)),
    ] {
        let h = harness(&format!("/Outlines 9 0 R /PageMode /FullScreen {prefs}"));
        assert_eq!(h.state().right, want, "{prefs}");
    }
    // Not in full screen, a document with an outline still opens on it.
    let h = harness("/Outlines 9 0 R");
    assert_eq!(h.state().right, Some(RightPanel::Bookmarks));
}
