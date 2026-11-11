//! Scan & OCR ▸ Enhance scanned file in the real shell (egui_kittest): the dialog, the run and
//! its report, and that the page keeps its comment while its scan is cleaned in place.

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use pdfcraft_engine::{Edit, NewAnnotation, NoteIcon, Session, Shape, Style};
use pdfcraft_ui_egui::PdfCraftApp;

/// A two-page PDF: page 1 a grey scan with a darker gutter and text bars (and a sticky note),
/// page 2 the same scan again.
fn scan() -> Vec<u8> {
    let img = image::GrayImage::from_fn(300, 400, |x, y| {
        let light = 0.55 + 0.45 * (x as f32 / 60.0).min(1.0);
        let ink = if (60..360).contains(&y) && (y - 60) % 40 < 6 && (60..240).contains(&x) { 0.12 } else { 0.78 };
        image::Luma([(ink * light * 255.0) as u8])
    });
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    let mut s = Session::new();
    let bytes = s.create_from_images(&[("a.png".into(), png.clone()), ("b.png".into(), png)]).unwrap();
    let id = s.open("scan.pdf", None, bytes, None).unwrap();
    let shape = Shape::Note { at: [20.0, 20.0], icon: NoteIcon::Comment };
    let note = NewAnnotation { page: 0, style: Style::default_for(&shape), shape, contents: "Keep me".into(), author: "Tester".into() };
    s.apply(id, Edit::AddAnnotation(note)).unwrap();
    s.save_bytes(id).unwrap().to_vec()
}

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("scan.pdf", None, scan()).unwrap();
        app.run_inline = true;
        app
    });
    h.run_steps(4);
    h
}

#[test]
fn enhance_scanned_file_cleans_the_chosen_pages_and_says_what_it_did() {
    let mut h = harness();
    let id = h.state().active_ids().unwrap().1;
    assert!(h.state().session.get(id).unwrap().comment_props(0, 0).is_some());
    assert!(h.state_mut().execute("ocr.enhance"));
    h.run_steps(2);
    assert_eq!(h.query_all_by_label("Background removal").count(), 2, "the label and the slider it names");
    h.get_by_label("Edge shadow removal");
    h.get_by_label("Despeckle");
    h.get_by_role_and_label(egui::accesskit::Role::RadioButton, "Current page").click();
    h.run_steps(1);
    h.get_by_label("Enhance").click();
    h.run_steps(3);
    let app = h.state();
    assert!(app.scan_clean_run.is_none() && app.dialog.is_none(), "finished and closed");
    let toast = app.toast.clone().unwrap().0;
    assert_eq!(toast, "Cleaned up the scan on 1 page.");
    let doc = app.session.get(id).unwrap();
    assert_eq!(doc.can_undo(), Some("Enhance scanned file"));
    assert_eq!(doc.info.pages.len(), 2);
    assert_eq!(doc.comment_props(0, 0).map(|p| p.author), Some("Tester".to_string()), "the sticky note stays");
}

#[test]
fn nothing_to_enhance_is_said_not_done() {
    let mut h = harness();
    let id = h.state().active_ids().unwrap().1;
    h.state_mut().scan_clean_draft.settings = pdfcraft_engine::scan_clean::ScanCleanSettings {
        level: 0,
        edge_shadows: false,
        despeckle: false,
        ..pdfcraft_engine::scan_clean::ScanCleanSettings::default()
    };
    assert!(h.state_mut().execute("ocr.enhance"));
    h.run_steps(2);
    // With every filter off the button is disabled; starting anyway changes nothing.
    h.state_mut().start_scan_clean();
    h.run_steps(2);
    let app = h.state();
    assert!(app.toast.clone().unwrap().0.starts_with("No scanned pages were cleaned up."), "{:?}", app.toast);
    assert_eq!(app.session.get(id).unwrap().can_undo(), None);
}

/// A PDF of 96 x 96 pages, each a halftone scan (a one-bit picture of its own: 20 % dots as paper,
/// a block of 70 % dots as ink, uncompressed) or, where `scans` says false, an empty page.
fn halftone(scans: &[bool]) -> Vec<u8> {
    let (w, h) = (96usize, 96usize);
    let mut bits = vec![0u8; w / 8 * h];
    for y in 0..h {
        for x in 0..w {
            let ink = (30..66).contains(&x) && (30..66).contains(&y);
            if (x * 7 + y * 3) % 10 >= if ink { 7 } else { 2 } {
                bits[y * w / 8 + x / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    let n = scans.len();
    let kids: String = (0..n).map(|i| format!("{} 0 R ", 3 + i)).collect();
    let mut pdf = format!(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [{kids}] /Count {n} >> endobj
"
    )
    .into_bytes();
    let mut image = 3 + 2 * n;
    for (i, scan) in scans.iter().enumerate() {
        let res = if *scan { format!("/Resources << /XObject << /Im0 {image} 0 R >> >>") } else { "/Resources << >>".to_string() };
        pdf.extend(
            format!(
                "{} 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 96 96] {res} /Contents {} 0 R >> endobj
",
                3 + i,
                3 + n + i
            )
            .as_bytes(),
        );
        image += usize::from(*scan);
    }
    for (i, scan) in scans.iter().enumerate() {
        let content = if *scan { "q 96 0 0 96 0 0 cm /Im0 Do Q" } else { "q Q" };
        pdf.extend(
            format!(
                "{} 0 obj << /Length {} >> stream
{content}
endstream endobj
",
                3 + n + i,
                content.len()
            )
            .as_bytes(),
        );
    }
    for (image, _) in (3 + 2 * n..).zip(scans.iter().filter(|s| **s)) {
        pdf.extend(format!("{image} 0 obj << /Type /XObject /Subtype /Image /Width 96 /Height 96 /ColorSpace /DeviceGray /BitsPerComponent 1 /Length 1152 >> stream
").as_bytes());
        pdf.extend_from_slice(&bits);
        pdf.extend_from_slice(
            b"
endstream endobj
",
        );
    }
    pdf.extend_from_slice(
        b"trailer << /Root 1 0 R >>
%%EOF",
    );
    pdf
}

fn halftone_harness(scans: &[bool]) -> Harness<'static, PdfCraftApp> {
    let bytes = halftone(scans);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("halftone.pdf", None, bytes.clone()).unwrap();
        app.run_inline = true;
        app
    });
    h.run_steps(4);
    h
}

fn enhance(h: &mut Harness<'static, PdfCraftApp>) {
    assert!(h.state_mut().execute("ocr.enhance"));
    h.run_steps(2);
    h.get_by_label("Enhance").click();
    h.run_steps(4);
}

/// The result window's Next page button (the toolbar has one of its own, which comes first).
fn next_button<'a>(h: &'a Harness<'static, PdfCraftApp>) -> egui_kittest::Node<'a> {
    h.query_all_by_label("Next page").last().expect("a Next page button")
}

fn current_page(h: &Harness<'static, PdfCraftApp>) -> usize {
    h.state().views[h.state().active.unwrap()].current
}

#[test]
fn a_restored_halftone_scan_opens_a_movable_result_window_whose_sliders_redo_the_enhancement() {
    let mut h = halftone_harness(&[true]);
    let id = h.state().active_ids().unwrap().1;
    enhance(&mut h);
    assert_eq!(h.state().dialog, Some(pdfcraft_ui_egui::Dialog::EnhanceScanResult), "the result window opens");
    h.get_by_label_contains("1-bit halftone image format was detected on page 1 and was enhanced using the Halftone Restore optimizing tool");
    h.get_by_label_contains("Use the sliders below to customize the optimization further, otherwise, click 'Close'.");
    for slider in ["Text weight", "Text contrast", "Background clean-up"] {
        assert_eq!(h.query_all_by_label(slider).count(), 2, "{slider}: the label and the slider it names");
    }
    // The page's own scan is what the sliders act on, and the current page is the default scope.
    assert!(!h.state().scan_clean_result.as_ref().unwrap().all);
    assert!(next_button(&h).accesskit_node().is_disabled(), "the only restored page has no next");
    // Letting a slider go enhances again from the original: one step in the history, other pixels.
    let before = h.state_mut().session.save_bytes(id).unwrap().to_vec();
    // The run is repeated on the pages it was asked for (all), so other scans stay cleaned.
    assert!(h.state().scan_clean_result.as_ref().unwrap().requested.is_empty());
    let result = h.state_mut().scan_clean_result.as_mut().unwrap();
    result.tunings[0].1.weight = 100;
    result.pending = true; // as a slider moved from the keyboard leaves it
    h.run_steps(4);
    let app = h.state_mut();
    assert_eq!(app.session.get(id).unwrap().can_undo(), Some("Enhance scanned file"));
    assert_eq!(app.session.get(id).unwrap().undo_depth(), 1, "the first enhancement was undone, not stacked");
    assert_ne!(app.session.save_bytes(id).unwrap().to_vec(), before, "the new setting changed the picture");
    h.get_by_label("Close").click();
    h.run_steps(2);
    assert!(h.state().dialog.is_none() && h.state().scan_clean_result.is_none());
}

#[test]
fn the_result_window_walks_the_restored_pages_and_only_ever_enhances_those() {
    // Pages 1 and 3 are scans, page 2 is empty, page 4 a scan again.
    let mut h = halftone_harness(&[true, false, true, true]);
    h.state_mut().views[0].go_to_page(1);
    enhance(&mut h);
    assert_eq!(current_page(&h), 0, "the first restored page is shown");
    assert_eq!(h.state().scan_clean_result.as_ref().unwrap().pages, vec![0, 2, 3]);
    h.get_by_label_contains("on pages 1, 3, 4");
    h.get_by_label("Apply to current page");
    h.get_by_label("Apply to all");
    next_button(&h).click();
    h.run_steps(2);
    assert_eq!(current_page(&h), 2, "Next page skips the page that is no scan");
    next_button(&h).click();
    h.run_steps(2);
    assert_eq!(current_page(&h), 3);
    assert!(next_button(&h).accesskit_node().is_disabled(), "greyed out at the last restored page");
    // A slider acts on the page showing alone, or, with Apply to all, on every restored page.
    let tunings =
        |h: &Harness<'static, PdfCraftApp>| h.state().scan_clean_result.as_ref().unwrap().tunings.iter().map(|(_, t)| t.weight).collect::<Vec<_>>();
    assert_eq!(tunings(&h), vec![50, 50, 50]);
    h.get_by_label("Apply to all").click();
    h.run_steps(2);
    assert!(h.state().scan_clean_result.as_ref().unwrap().all);
    h.get_by_label("Apply to current page").click();
    h.run_steps(2);
    assert!(!h.state().scan_clean_result.as_ref().unwrap().all, "the two choices exclude each other");
    // The retune job covers the restored pages only: never page 2.
    let job = h
        .state()
        .session
        .scan_clean_job(h.state().active_ids().unwrap().1, &h.state().scan_clean_result.as_ref().unwrap().pages, Default::default())
        .unwrap();
    assert_eq!(job.total(), 3, "three images, not four pages");
}

/// The sliders themselves: moving one from the keyboard tunes the page showing and enhances it
/// again as one step, and with Apply to all it tunes every restored page.
#[test]
fn moving_a_slider_tunes_the_page_showing_or_all_of_them_and_enhances_again() {
    let mut h = halftone_harness(&[true, false, true, true]);
    let id = h.state().active_ids().unwrap().1;
    enhance(&mut h);
    let tunings = |h: &Harness<'static, PdfCraftApp>| {
        let o = h.state().scan_clean_result.as_ref().unwrap();
        o.tunings.iter().map(|(_, t)| (t.weight, t.contrast, t.cleanup)).collect::<Vec<_>>()
    };
    assert_eq!(tunings(&h), vec![(50, 50, 50); 3]);
    let before = h.state_mut().session.save_bytes(id).unwrap().to_vec();
    h.get_by_role_and_label(egui::accesskit::Role::Slider, "Text weight").focus();
    h.run_steps(2);
    h.key_press(egui::Key::ArrowRight);
    h.run_steps(6);
    assert_eq!(tunings(&h), vec![(51, 50, 50), (50, 50, 50), (50, 50, 50)], "page 1 alone, the page showing");
    let app = h.state_mut();
    assert_eq!(app.session.get(id).unwrap().undo_depth(), 1, "enhanced again as the same one step");
    assert_ne!(app.session.save_bytes(id).unwrap().to_vec(), before, "with another picture");
    let result = app.scan_clean_result.as_ref().unwrap();
    assert!(!result.pending && result.again.is_empty(), "and nothing is left to do");
    assert_eq!(app.dialog, Some(pdfcraft_ui_egui::Dialog::EnhanceScanResult), "the window stays for the next change");
    // Apply to all: one slider moves every restored page.
    h.get_by_label("Apply to all").click();
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::Slider, "Text contrast").focus();
    h.run_steps(2);
    h.key_press(egui::Key::ArrowLeft);
    h.run_steps(6);
    assert_eq!(tunings(&h), vec![(51, 49, 50); 3], "every restored page takes the sliders as they stand");
    assert_eq!(h.state().session.get(id).unwrap().undo_depth(), 1);
}

/// The window is there for the page beside it to be judged: the page still zooms and scrolls
/// while it is open, and Escape closes it.
#[test]
fn the_page_zooms_beside_the_result_window_and_escape_closes_it() {
    let mut h = halftone_harness(&[true]);
    enhance(&mut h);
    assert_eq!(h.state().dialog, Some(pdfcraft_ui_egui::Dialog::EnhanceScanResult));
    let zoom = h.state().views[0].zoom;
    // Over the page, clear of the window.
    h.hover_at(egui::pos2(1000.0, 600.0));
    h.run_steps(1);
    h.event(egui::Event::Zoom(1.5));
    h.run_steps(4);
    assert!((h.state().views[0].zoom - zoom * 1.5).abs() < 0.01, "zoomed from {zoom} to {}", h.state().views[0].zoom);
    assert_eq!(h.state().dialog, Some(pdfcraft_ui_egui::Dialog::EnhanceScanResult), "and the window is still there");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(h.state().dialog.is_none() && h.state().scan_clean_result.is_none());
}

/// In full screen a click on the page turns it, but not beside the result window: the page there
/// scrolls and zooms, and presses on it stay off.
#[test]
fn full_screen_clicks_beside_the_result_window_do_not_turn_the_page() {
    let mut h = halftone_harness(&[true, true]);
    assert!(h.state_mut().execute("view.full_screen"));
    h.run_steps(2);
    assert!(h.state().full_screen, "full screen");
    enhance(&mut h);
    assert_eq!(h.state().dialog, Some(pdfcraft_ui_egui::Dialog::EnhanceScanResult));
    assert_eq!(current_page(&h), 0);
    // Over the page, clear of the window.
    let at = egui::pos2(1000.0, 600.0);
    h.hover_at(at);
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
    }
    h.run_steps(3);
    assert_eq!(current_page(&h), 0, "a click beside the window turned the page forward");
    h.state_mut().views[0].go_to_page(1);
    h.run_steps(3);
    assert_eq!(current_page(&h), 1);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed, modifiers: Default::default() });
    }
    h.run_steps(3);
    assert_eq!(current_page(&h), 1, "a right-click beside the window turned the page back");
    assert_eq!(h.state().dialog, Some(pdfcraft_ui_egui::Dialog::EnhanceScanResult), "and the window is still there");
}

/// Restore halftone scans is a choice in the dialog: unticked, a dotted scan is left to the other
/// filters and no result window opens.
#[test]
fn unticking_restore_halftone_scans_leaves_a_dotted_scan_as_it_is() {
    let mut h = halftone_harness(&[true]);
    let id = h.state().active_ids().unwrap().1;
    assert!(h.state_mut().execute("ocr.enhance"));
    h.run_steps(2);
    assert!(h.state().scan_clean_draft.settings.restore_halftones, "on to begin with");
    h.get_by_label("Restore halftone scans").click();
    h.run_steps(2);
    assert!(!h.state().scan_clean_draft.settings.restore_halftones);
    h.get_by_label("Enhance").click();
    h.run_steps(4);
    assert!(h.state().dialog.is_none() && h.state().scan_clean_result.is_none(), "no result window");
    assert_eq!(h.state().session.get(id).unwrap().undo_depth(), 0, "and nothing changed");
}
