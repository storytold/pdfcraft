//! Viewer conveniences: close all, revert, fit height, view history, select all, find options,
//! cover page.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{Dialog, PdfCraftApp};

/// `n` pages of 200×300; page i says "Page i+1" plus "page" and "Pages" for find tests.
fn fixture(n: usize) -> Vec<u8> {
    let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 200 300] >>", kids.join(" ")));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
    for i in 0..n {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * i));
        let body = format!("BT /F1 18 Tf 20 150 Td (Page {} pages PAGE) Tj ET", i + 1);
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("a.pdf", None, fixture(5)).unwrap();
        app.open_bytes("b.pdf", None, fixture(2)).unwrap();
        app
    });
    for _ in 0..40 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h
}

#[test]
fn close_all_asks_only_for_changed_documents() {
    let mut h = harness();
    // Change the active document (b.pdf).
    h.state_mut().apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
    h.state_mut().execute("file.close_all");
    h.run_steps(2);
    assert_eq!(h.state().views.len(), 1, "the clean document closed at once");
    assert_eq!(h.state().close_request, Some(pdfcraft_ui_egui::CloseRequest::All), "b.pdf asks to be saved");
    h.get_by_label_contains("Save changes");
}

#[test]
fn revert_after_confirming() {
    let mut h = harness();
    h.state_mut().apply_edit(pdfcraft_engine::Edit::DeletePages { pages: vec![0] });
    assert!(h.state_mut().execute("file.revert"));
    h.run_steps(2);
    assert_eq!(h.state().dialog, Some(Dialog::Revert));
    h.get_all_by_label("Revert").last().expect("the button").click();
    h.run_steps(3);
    let s = h.state();
    let d = s.session.get(s.views[1].id).unwrap();
    assert_eq!((d.info.pages.len(), d.dirty), (2, false));
}

#[test]
fn view_history_select_all_and_find_options() {
    let mut h = harness();
    h.state_mut().active = Some(0);
    h.run_steps(2);
    h.state_mut().views[0].go_to_page(3);
    h.run_steps(2);
    h.state_mut().views[0].go_to_page(1);
    h.run_steps(2);
    assert!(h.state_mut().views[0].view_history(false));
    assert_eq!(h.state().views[0].current, 3, "previous view");
    assert!(h.state_mut().views[0].view_history(false));
    assert_eq!(h.state().views[0].current, 0);
    assert!(h.state_mut().views[0].view_history(true));
    assert_eq!(h.state().views[0].current, 3, "next view");
    // Select all on the current page (once its text is known).
    for _ in 0..40 {
        h.run_steps(2);
        if h.state_mut().views[0].select_all() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("Page 4 pages PAGE"));
    // Find: "page" matches three times a page; whole words, case-sensitive narrows it.
    h.state_mut().views[0].open_find();
    h.state_mut().views[0].find.as_mut().unwrap().query = "page".into();
    h.state_mut().views[0].rerun_find();
    for _ in 0..60 {
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(5));
        if h.state().views[0].find.as_ref().unwrap().matches.len() >= 15 {
            break;
        }
    }
    assert_eq!(h.state().views[0].find.as_ref().unwrap().matches.len(), 15);
    {
        let f = h.state_mut().views[0].find.as_mut().unwrap();
        f.whole_words = true;
        f.case_sensitive = true;
    }
    h.state_mut().views[0].rerun_find();
    h.run_steps(2);
    assert_eq!(h.state().views[0].find.as_ref().unwrap().matches.len(), 0, "\"page\" alone, lower case, appears nowhere");
    h.state_mut().views[0].find.as_mut().unwrap().query = "PAGE".into();
    h.state_mut().views[0].rerun_find();
    h.run_steps(2);
    assert_eq!(h.state().views[0].find.as_ref().unwrap().matches.len(), 5);
}

#[test]
fn layouts_fit_height_labels_and_system_theme() {
    use pdfcraft_ui_egui::canvas::{Fit, PageLayout};
    let mut h = harness();
    h.state_mut().active = Some(0);
    // Fit height: the page's height fills the view (less the margins).
    h.state_mut().views[0].fit = Fit::Height;
    h.run_steps(4);
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let vp = h.state().views[0].viewport_rect();
    assert!(r.height() > vp.height() * 0.85 && r.top() >= vp.top() && r.bottom() <= vp.bottom() + 1.0, "page {r:?} in {vp:?}");
    // Two-page view scrolls continuously; a cover puts page 1 alone on the right.
    h.state_mut().views[0].fit = Fit::Width;
    h.state_mut().views[0].layout = PageLayout::TwoUp;
    h.run_steps(4);
    let (a, b) = (h.state().views[0].page_screen_rect(0).unwrap(), h.state().views[0].page_screen_rect(1).unwrap());
    assert!((a.top() - b.top()).abs() < 1.0 && b.left() > a.right(), "side by side");
    h.state_mut().views[0].cover = true;
    h.run_steps(4);
    let (a, b, c) = (
        h.state().views[0].page_screen_rect(0).unwrap(),
        h.state().views[0].page_screen_rect(1).unwrap(),
        h.state().views[0].page_screen_rect(2).unwrap(),
    );
    assert!(a.left() > vp.center().x - 1.0, "the cover sits on the right");
    assert!(b.top() > a.bottom() && (b.top() - c.top()).abs() < 1.0, "then pairs 2–3");
    // Page labels in the page box.
    h.state_mut().apply_edit(pdfcraft_engine::Edit::NumberPages {
        from: 0,
        to: 1,
        style: pdfcraft_engine::LabelStyle::LowerRoman,
        prefix: String::new(),
        first: 1,
    });
    h.run_steps(2);
    let labels: Vec<String> = {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().info.pages.iter().map(|p| p.label.clone()).collect()
    };
    assert_eq!(labels[1], "ii");
    assert!(h.state_mut().views[0].go_to_typed("ii", &labels));
    assert_eq!(h.state().views[0].current, 1);
    assert!(!h.state_mut().views[0].go_to_typed("xx", &labels));
    // Follow the system theme.
    h.state_mut().set_option("theme", "system").unwrap();
    assert!(h.state().follow_system_theme);
}

#[test]
fn two_page_view_steps_a_spread_at_a_time() {
    // #70: ⌘→ went to the right-hand page of the same spread, so the view never moved.
    use egui::{Key, Modifiers};
    use pdfcraft_ui_egui::canvas::{Fit, PageLayout};
    let mut h = harness();
    h.state_mut().active = Some(0);
    h.state_mut().views[0].fit = Fit::Width;
    h.state_mut().views[0].layout = PageLayout::TwoUp;
    h.run_steps(4);
    let step = |h: &mut Harness<'static, PdfCraftApp>, key| {
        h.key_press_modifiers(Modifiers::COMMAND, key);
        h.run_steps(4);
        h.state().views[0].current
    };
    let pages: Vec<usize> = [Key::ArrowRight, Key::ArrowRight, Key::ArrowRight, Key::ArrowLeft, Key::ArrowLeft].map(|k| step(&mut h, k)).into();
    assert_eq!(pages, [2, 4, 4, 2, 0], "spreads 1–2, 3–4, 5");
    // With a cover page the spreads are 1, 2–3, 4–5.
    h.state_mut().views[0].cover = true;
    h.run_steps(4);
    let pages: Vec<usize> = [Key::ArrowRight, Key::ArrowRight, Key::ArrowLeft, Key::ArrowLeft].map(|k| step(&mut h, k)).into();
    assert_eq!(pages, [1, 3, 1, 0]);
}

#[test]
fn damaged_files_say_they_were_repaired() {
    // No cross-reference table: the file is reconstructed.
    let damaged = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj
trailer << /Root 1 0 R >>
%%EOF"
        .to_vec();
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("damaged.pdf", None, damaged.clone()).unwrap();
        app
    });
    h.run_steps(4);
    h.get_by_label_contains("This file was damaged and has been repaired.");
    h.get_by_label("Details").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::Properties(pdfcraft_ui_egui::PropsTab::Advanced)));
    h.get_by_label("Repair log");
}

#[test]
fn initial_view_is_edited_and_honoured_on_open() {
    use pdfcraft_engine::{InitialLayout, Magnification, Navigation};
    let mut h = harness();
    h.state_mut().dialog = Some(Dialog::Properties(pdfcraft_ui_egui::PropsTab::InitialView));
    h.run_steps(2);
    h.get_by_label("Open to page");
    {
        let (_, v) = h.state_mut().view_draft.as_mut().expect("seeded");
        v.navigation = Navigation::Bookmarks;
        v.layout = InitialLayout::TwoUpCoverPage;
        v.magnification = Magnification::FitWidth;
        v.page = 1;
        v.language = Some("de-DE".into());
    }
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
    let s = h.state();
    let id = s.views[s.active.unwrap()].id;
    let v = s.session.get(id).unwrap().initial_view();
    assert_eq!((v.navigation, v.layout, v.page, v.language.as_deref()), (Navigation::Bookmarks, InitialLayout::TwoUpCoverPage, 1, Some("de-DE")));
    assert_eq!(s.session.get(id).unwrap().can_undo(), Some("Change document properties"));
    // Opening the saved file follows it.
    let bytes = s.session.save_bytes(id).unwrap();
    let mut app = PdfCraftApp::new();
    app.open_bytes("again.pdf", None, bytes.to_vec()).unwrap();
    let view = &app.views[0];
    assert_eq!((view.current, view.cover, app.right), (1, true, Some(pdfcraft_ui_egui::RightPanel::Bookmarks)));
}

fn drag(h: &mut Harness<'static, PdfCraftApp>, a: egui::Pos2, b: egui::Pos2) {
    h.hover_at(a);
    h.run_steps(1);
    h.drag_at(a);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(a + (b - a) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    h.drop_at(b);
    h.run_steps(3);
}

#[test]
fn marquee_zoom_and_snapshot() {
    let mut h = harness();
    let i = h.state().active.unwrap();
    // Text "Page 2 pages PAGE" sits at y 150 (of 300) from x 20.
    let r = h.state().views[i].page_screen_rect(0).expect("on screen");
    let at = |x: f32, y: f32| egui::pos2(r.left() + x / 200.0 * r.width(), r.top() + (300.0 - y) / 300.0 * r.height());
    // Snapshot of the text line.
    h.state_mut().system_clipboard = false;
    assert!(h.state_mut().execute("edit.snapshot"));
    drag(&mut h, at(15.0, 175.0), at(180.0, 140.0));
    let (w, hgt, px) = h.state().last_snapshot.clone().expect("a snapshot");
    assert!(w > 50 && hgt > 10, "{w} × {hgt}");
    assert!(px.as_chunks::<4>().0.iter().any(|p| p[0] < 100), "the text is in it");
    // Marquee zoom on a small area zooms in, centred on it.
    let before = h.state().views[i].zoom;
    h.state_mut().set_option("quick", "marquee-zoom").unwrap();
    drag(&mut h, at(20.0, 160.0), at(60.0, 140.0));
    h.run_steps(4);
    let after = h.state().views[i].zoom;
    assert!(after > before * 2.0, "{before} → {after}");
}

#[test]
fn fit_visible_zooms_to_the_content_width() {
    let mut h = harness();
    let i = h.state().active.unwrap();
    let before = h.state().views[i].zoom;
    assert!(h.state_mut().execute("view.fit_visible"));
    for _ in 0..4 {
        h.run_steps(2);
    }
    let v = &h.state().views[i];
    assert!(v.zoom > before * 1.2, "{before} → {}", v.zoom);
    // The text starts 20 pt in from the page's left; that edge is now near the window's left.
    let r = v.page_screen_rect(v.current).expect("on screen");
    let ink_left = r.left() + 20.0 / 200.0 * r.width();
    let vp = v.viewport_rect();
    assert!((ink_left - vp.left() - 16.0).abs() < 12.0, "ink at {ink_left}, viewport {vp:?}");
}

#[test]
fn tab_and_window_show_the_document_title_when_asked() {
    use pdfcraft_engine::Edit;
    let mut h = harness();
    h.run_steps(2);
    assert_eq!(h.state().window_title, "b.pdf — PdfCraft");
    {
        let s = h.state_mut();
        let id = s.views[s.active.unwrap()].id;
        s.session.apply(id, Edit::SetInfo { key: "Title".into(), value: "Quarterly report".into() }).unwrap();
        let mut v = s.session.get(id).unwrap().initial_view();
        v.display_title = true;
        s.session.apply(id, Edit::SetInitialView(Box::new(v))).unwrap();
    }
    h.run_steps(2);
    assert_eq!(h.state().window_title, "Quarterly report — PdfCraft");
    h.get_by_label_contains("Quarterly report");
}

#[test]
fn an_earlier_revision_opens_from_document_properties() {
    use pdfcraft_engine::Edit;
    let mut h = harness();
    {
        let s = h.state_mut();
        let id = s.views[s.active.unwrap()].id;
        s.session.apply(id, Edit::SetInfo { key: "Title".into(), value: "Second".into() }).unwrap();
        let bytes = s.session.save_bytes(id).unwrap();
        s.open_bytes("updated.pdf", None, bytes.to_vec()).unwrap();
    }
    h.run_steps(2);
    h.state_mut().dialog = Some(Dialog::Properties(pdfcraft_ui_egui::PropsTab::Advanced));
    h.run_steps(2);
    h.run_steps(2);
    h.get_by_label("View revision 1").click();
    h.run_steps(3);
    let s = h.state();
    assert!(s.dialog.is_none(), "{:?} {:?}", s.dialog, s.session.get(s.views[s.active.unwrap()].id).unwrap().name);
    let doc = s.session.get(s.views[s.active.unwrap()].id).unwrap();
    assert_eq!(doc.name, "updated (revision 1).pdf");
    assert_eq!(doc.info_value("Title"), None, "as it was before the update");
}

#[test]
fn advanced_search_lists_results_with_context() {
    let mut h = harness();
    let i = h.state().active.unwrap();
    assert!(h.state_mut().execute("edit.advanced_search"));
    h.run_steps(2);
    h.get_by_label("What word or phrase would you like to search for?");
    h.state_mut().views[i].find.as_mut().unwrap().query = "pages".into();
    for _ in 0..40 {
        h.run_steps(2);
        if h.state().views[i].find.as_ref().is_some_and(|f| f.matches.len() == 2) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.run_steps(2);
    h.get_by_label("2 instances");
    h.get_by_label("Page 2");
    // A result shows its context; clicking it goes there.
    h.get_by_label_contains("Page 2 pages PAGE").click();
    h.run_steps(3);
    let v = &h.state().views[i];
    assert_eq!((v.find.as_ref().unwrap().current, v.current), (Some(1), 1));
    // Closing the panel moves the search to the find bar.
    h.state_mut().right = None;
    h.run_steps(2);
    assert!(!h.state().views[i].find.as_ref().unwrap().in_panel);
}

#[test]
fn optimizer_audits_space_usage() {
    let mut h = harness();
    assert!(h.state_mut().execute("optimize.advanced"));
    h.run_steps(2);
    h.get_by_label("Audit space usage…").click();
    h.run_steps(3);
    h.get_by_label("Space Audit");
    h.get_by_label("Content Streams");
    h.get_by_label("Total");
    h.get_by_label("OK").click();
    h.run_steps(2);
    assert_eq!(h.state().dialog, Some(Dialog::Optimize), "back to the optimizer");
}
