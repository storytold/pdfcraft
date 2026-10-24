//! Viewer conveniences: close all, revert, fit height, view history, select all, find options,
//! cover page, a scroll position per tab.

use egui_kittest::Harness;
use egui_kittest::kittest::{By, Queryable};
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
        app.set_option("language", "en").unwrap();
        app.open_bytes("a.pdf", None, fixture(5)).unwrap();
        app.open_bytes("b.pdf", None, fixture(2)).unwrap();
        app
    });
    // Background rendering and text extraction can take seconds on a loaded CI runner (FreeBSD VM).
    crate::support::settle(&mut h);
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
    // Select all on the current page (once its text is known), one page at a time: scrolling
    // continuously it takes the whole document's text (#2639).
    h.state_mut().views[0].set_continuous(false);
    // Background rendering and text extraction can take seconds on a loaded CI runner (FreeBSD VM).
    for _ in 0..3000 {
        h.run_steps(2);
        if h.state_mut().views[0].select_all() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("Page 4 pages PAGE"));
    h.state_mut().views[0].select_text(3, 0, 0);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run_steps(2);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("Page 4 pages PAGE"), "the viewer shortcut still selects text");
    assert!(h.state().views[0].selected.is_empty(), "the viewer shortcut doesn't select pages");
    // Find: "page" matches three times a page; whole words, case-sensitive narrows it.
    h.state_mut().views[0].open_find();
    h.state_mut().views[0].find.as_mut().unwrap().query = "page".into();
    h.state_mut().views[0].rerun_find();
    // Find runs in the background: wait for it by a deadline, not a frame count, which a loaded
    // machine (the whole workspace testing at once) can use up before the last page is searched.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while std::time::Instant::now() < deadline && h.state().views[0].find.as_ref().unwrap().matches.len() < 15 {
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(5));
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
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while std::time::Instant::now() < deadline && h.state().views[0].find.as_ref().unwrap().matches.len() < 5 {
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(h.state().views[0].find.as_ref().unwrap().matches.len(), 5);
}

#[test]
fn find_shortcuts_advance_and_retreat_matches() {
    let mut h = harness();
    h.state_mut().active = Some(0);
    h.run_steps(2);
    h.state_mut().views[0].open_find();
    h.state_mut().views[0].find.as_mut().unwrap().query = "page".into();
    h.state_mut().views[0].rerun_find();
    // Find runs in the background: wait by a deadline, not a frame count, which a loaded machine
    // can use up before the last page is searched.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(5));
        if h.state().views[0].find.as_ref().unwrap().matches.len() >= 15 {
            break;
        }
    }
    let n = h.state().views[0].find.as_ref().unwrap().matches.len();
    assert_eq!(n, 15);
    // Surrender text edit focus so canvas shortcut handler receives key events.
    h.state_mut().views[0].find.as_mut().unwrap().focus = false;
    h.ctx.memory_mut(|m| m.surrender_focus(egui::Id::new("find-input")));
    h.run_steps(2);
    let start = h.state().views[0].find.as_ref().unwrap().current.unwrap();
    // ⌘G advances to next match.
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::G);
    h.run_steps(2);
    assert_eq!(h.state().views[0].find.as_ref().unwrap().current, Some((start + 1) % n));
    // ⇧⌘G retreats to previous match.
    h.key_press_modifiers(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::G);
    h.run_steps(2);
    assert_eq!(h.state().views[0].find.as_ref().unwrap().current, Some(start));
    // ⇧⌘G again wraps backwards.
    h.key_press_modifiers(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::G);
    h.run_steps(2);
    assert_eq!(h.state().views[0].find.as_ref().unwrap().current, Some((start + n - 1) % n));
}

/// #2669: ⌘G and ⇧⌘G step through the matches while the find field still has the focus.
#[test]
fn find_shortcuts_work_from_the_find_field() {
    let mut h = harness();
    h.state_mut().active = Some(0);
    h.run_steps(2);
    h.state_mut().views[0].open_find();
    h.state_mut().views[0].find.as_mut().unwrap().query = "page".into();
    h.state_mut().views[0].rerun_find();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(5));
        if h.state().views[0].find.as_ref().unwrap().matches.len() >= 15 {
            break;
        }
    }
    let n = h.state().views[0].find.as_ref().unwrap().matches.len();
    assert_eq!(n, 15);
    assert!(h.ctx.egui_wants_keyboard_input(), "the find field has the focus");
    let start = h.state().views[0].find.as_ref().unwrap().current.unwrap();
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::G);
    h.run_steps(2);
    assert_eq!(h.state().views[0].find.as_ref().unwrap().current, Some((start + 1) % n));
    h.key_press_modifiers(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::G);
    h.run_steps(2);
    assert_eq!(h.state().views[0].find.as_ref().unwrap().current, Some(start));
    assert!(h.ctx.egui_wants_keyboard_input(), "and keeps it");
}

#[test]
fn marking_while_searching_does_not_jitter_or_reset_search_state() {
    let mut h = harness();
    h.state_mut().active = Some(0);
    h.run_steps(2);
    h.state_mut().views[0].open_find();
    h.state_mut().views[0].find.as_mut().unwrap().query = "page".into();
    h.state_mut().views[0].rerun_find();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(5));
        if h.state().views[0].find.as_ref().unwrap().matches.len() >= 15 {
            break;
        }
    }
    assert_eq!(h.state().views[0].find.as_ref().unwrap().matches.len(), 15);
    let current_before = h.state().views[0].find.as_ref().unwrap().current;
    assert!(current_before.is_some());
    // Clear any pending goto from initial find
    h.run_steps(2);
    assert!(h.state().views[0].goto.is_none());

    // User draws or marks an annotation on page 0
    let shape = pdfcraft_engine::Shape::Ink { strokes: vec![vec![[10.0, 10.0], [20.0, 20.0]]] };
    let stroke = pdfcraft_engine::Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
        page: 0,
        style: pdfcraft_engine::Style::default_for(&shape),
        shape,
        contents: String::new(),
        author: "tester".into(),
    });
    h.state_mut().apply_edit(stroke);

    // Right after applying the edit, search matches and current match must still be valid
    assert_eq!(h.state().views[0].find.as_ref().unwrap().current, current_before, "applying an annotation must not wipe current search match");

    // Let the canvas process the reload
    let reload_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < reload_deadline {
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(h.state().views[0].goto.is_none(), "page reload after annotation must not set goto / trigger display jitter");
        if h.state().views[0].glyph_screen_pos(0, 0).is_some() {
            break;
        }
    }
    assert_eq!(h.state().views[0].find.as_ref().unwrap().current, current_before);
    assert_eq!(h.state().views[0].find.as_ref().unwrap().matches.len(), 15);

    // If user is viewing page 3 and reruns find, the selected match must be on page 3 (index 9), not page 0
    h.state_mut().views[0].current = 3;
    h.state_mut().views[0].rerun_find();
    let current_p3 = h.state().views[0].find.as_ref().unwrap().current;
    assert_eq!(current_p3, Some(9), "rerun_find must select the first match at or after the viewed page (page 3), but selected {:?}", current_p3);

    // Continuous drawing across multiple pages must never trigger viewport jumps or wipe search state
    h.run_steps(2);
    h.state_mut().views[0].goto = None;
    for p in 1..=4 {
        let shape = pdfcraft_engine::Shape::Ink { strokes: vec![vec![[15.0 * p as f64, 20.0], [30.0 * p as f64, 40.0]]] };
        let stroke = pdfcraft_engine::Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
            page: p,
            style: pdfcraft_engine::Style::default_for(&shape),
            shape,
            contents: String::new(),
            author: "tester".into(),
        });
        h.state_mut().apply_edit(stroke);
        h.run_steps(2);
        assert!(h.state().views[0].goto.is_none(), "drawing on page {p} must not set goto");
        assert_eq!(h.state().views[0].find.as_ref().unwrap().current, Some(9));
    }
    let multi_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < multi_deadline {
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(h.state().views[0].goto.is_none(), "multi-page reload after annotations must not set goto");
    }
    assert_eq!(h.state().views[0].find.as_ref().unwrap().current, Some(9));
    assert_eq!(h.state().views[0].find.as_ref().unwrap().matches.len(), 15);
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
    // Under the box, the physical position, since "ii" does not say which page it is (#2834).
    h.run_steps(2);
    h.get(By::new().label(&format!("2/{}", labels.len())));
    assert!(!h.state_mut().views[0].go_to_typed("xx", &labels));
    // Follow the system theme.
    h.state_mut().set_option("theme", "system").unwrap();
    assert_eq!(h.state().theme_preference, pdfcraft_ui_egui::theme::ThemePreference::System);
}

fn page_input_focused(h: &Harness<'static, PdfCraftApp>) -> bool {
    let focused = h.ctx.memory(|m| m.focused());
    let view = &h.state().views[h.state().active.unwrap()];
    // Also identifies the original editor when running this test-only patch on the baseline.
    focused == Some(egui::Id::new(("page-input", view.id.0))) || focused == Some(egui::Id::new("page-input"))
}

/// Exercise the real navigation editor, including its focus and Enter handling.
fn focus_page_input(h: &mut Harness<'static, PdfCraftApp>) {
    let text = h.state().views[h.state().active.unwrap()].page_input.clone();
    h.get(By::new().role(egui::accesskit::Role::TextInput).value(&text)).focus();
    h.run_steps(1);
    assert!(page_input_focused(h), "the actual page editor has focus");
}

fn type_page_input(h: &mut Harness<'static, PdfCraftApp>, text: &str) {
    focus_page_input(h);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run_steps(1);
    let old = h.state().views[h.state().active.unwrap()].page_input.clone();
    h.get(By::new().role(egui::accesskit::Role::TextInput).value(&old)).type_text(text);
    h.run_steps(2);
}

fn restart_page_labels(h: &mut Harness<'static, PdfCraftApp>) {
    use pdfcraft_engine::{Edit, LabelStyle};
    let session = std::mem::take(&mut h.state_mut().session);
    h.state_mut().session = session.with_clock(|| 1_700_000_000);
    h.state_mut().active = Some(0);
    for (from, to, style, first) in [(0, 1, LabelStyle::LowerRoman, 1), (2, 4, LabelStyle::Decimal, 1)] {
        h.state_mut().apply_edit(Edit::NumberPages { from, to, style, prefix: String::new(), first });
    }
    h.run_steps(3);
}

#[test]
fn page_label_input_keeps_current_page_when_submitted_unchanged() {
    let mut h = harness();
    restart_page_labels(&mut h);
    // Physical page 3 has logical label 1. The old editor showed 3, then interpreted it as
    // logical label 3 (physical page 5) when Enter was pressed without changing anything.
    h.state_mut().views[0].go_to_page(2);
    h.run_steps(3);
    focus_page_input(&mut h);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(h.state().views[0].current, 2, "unchanged Enter must stay on the current physical page");

    let id = h.state().views[0].id;
    let before = h.state().session.get(id).unwrap();
    let undo = before.can_undo().map(str::to_owned);
    let dirty = before.dirty;
    let before_bytes = h.state().session.save_bytes(id).unwrap();
    for (page, label) in ["i", "ii", "1", "2", "3"].into_iter().enumerate() {
        h.state_mut().views[0].go_to_page(page);
        h.run_steps(3);
        assert_eq!(h.state().views[0].page_input, label);
        focus_page_input(&mut h);
        h.key_press(egui::Key::Enter);
        h.run_steps(3);
        assert_eq!(h.state().views[0].current, page, "{label}");
        assert_eq!(h.state().views[0].page_input, label);
    }
    let after = h.state().session.get(id).unwrap();
    assert_eq!(after.can_undo().map(str::to_owned), undo);
    assert_eq!(after.dirty, dirty, "navigation does not edit the PDF");
    assert_eq!(h.state().session.save_bytes(id).unwrap(), before_bytes, "navigation preserves saved PDF bytes");
    let mut empty = pdfcraft_ui_egui::canvas::DocView::new(pdfcraft_engine::DocId(0), &Default::default(), Default::default());
    assert!(!empty.go_to_typed("1", &[]), "an empty view has no current page to retain");
}

#[test]
fn page_label_input_resolves_labels_numbers_and_restores_invalid_input() {
    use pdfcraft_engine::{Edit, LabelStyle};
    let mut h = harness();
    restart_page_labels(&mut h);
    h.state_mut().apply_edit(Edit::NumberPages { from: 4, to: 4, style: LabelStyle::Decimal, prefix: "A-".into(), first: 8 });
    h.run_steps(3);
    for (typed, page, displayed) in [("ii", 1, "ii"), ("2", 3, "2"), ("A-8", 4, "A-8"), ("3", 2, "1"), ("missing", 2, "1")] {
        type_page_input(&mut h, typed);
        h.key_press(egui::Key::Enter);
        h.run_steps(3);
        assert_eq!(h.state().views[0].current, page, "{typed}");
        assert_eq!(h.state().views[0].page_input, displayed, "{typed}");
    }
    // Scripted page selection remains physical and one-based even when a numeric label differs.
    h.state_mut().set_option("page", "2").unwrap();
    h.run_steps(3);
    assert_eq!((h.state().views[0].current, h.state().views[0].page_input.as_str()), (1, "ii"));

    h.state_mut().apply_edit(Edit::NumberPages { from: 0, to: 1, style: LabelStyle::None, prefix: "Appendix".into(), first: 1 });
    h.state_mut().views[0].go_to_page(1);
    h.run_steps(3);
    assert_eq!(h.state().views[0].page_input, "Appendix");
    focus_page_input(&mut h);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(h.state().views[0].current, 1, "a duplicate current label must not resolve to its first occurrence");
    h.state_mut().views[0].go_to_page(2);
    h.run_steps(3);
    type_page_input(&mut h, "Appendix");
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(h.state().views[0].current, 0, "a changed label keeps the existing first-match lookup");

    h.state_mut().views[0].go_to_page(0);
    h.state_mut().apply_edit(Edit::NumberPages { from: 0, to: 0, style: LabelStyle::None, prefix: String::new(), first: 1 });
    h.run_steps(3);
    assert_eq!(h.state().views[0].page_input, "1", "an empty label falls back to the physical number");
    focus_page_input(&mut h);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(h.state().views[0].current, 0, "the current fallback number must not jump to another page labelled 1");
}

#[test]
fn page_label_input_preserves_literal_whitespace_before_trimmed_lookup() {
    use pdfcraft_render::{DocInfo, PageInfo};
    use pdfcraft_ui_egui::canvas::DocView;
    // The importer trims label prefixes, so supply literal labels directly to the view.
    for label in [" 1", "1 "] {
        let info = DocInfo {
            pages: [label, "ii", "1"]
                .into_iter()
                .map(|label| PageInfo { width: 200.0, height: 300.0, label: label.into(), crop: [0.0, 0.0, 200.0, 300.0], rotation: 0 })
                .collect(),
            ..Default::default()
        };
        let labels: Vec<String> = info.pages.iter().map(|page| page.label.clone()).collect();
        let mut view = DocView::new(pdfcraft_engine::DocId(0), &info, Default::default());
        view.document_replaced(&info);
        assert_eq!(view.page_input, label, "the displayed label retains its literal whitespace");
        let unchanged = view.page_input.clone();
        assert!(view.go_to_typed(&unchanged, &labels));
        assert_eq!(view.current, 0, "unchanged spaces in {label:?} belong to the label");
        assert!(view.go_to_typed("1", &labels));
        assert_eq!(view.current, 2, "changed input keeps the existing trimmed label lookup");
    }
}

#[test]
fn page_label_input_focus_command_refreshes_same_frame_navigation_and_preserves_a_draft() {
    let mut h = harness();
    restart_page_labels(&mut h);
    assert_eq!(h.state().views[0].page_input, "i");
    // No frame between navigation and the real command: it must not focus the old label.
    h.state_mut().set_option("page", "3").unwrap();
    assert!(h.state_mut().execute("view.focus_page_input"));
    h.run_steps(3);
    assert!(page_input_focused(&h));
    assert_eq!(h.state().views[0].page_input, "1");
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(h.state().views[0].current, 2, "the focus command selects the new current label");

    type_page_input(&mut h, "draft");
    h.state_mut().set_option("page", "4").unwrap();
    assert!(h.state_mut().execute("view.focus_page_input"));
    h.run_steps(3);
    assert_eq!(h.state().views[0].current, 3);
    assert_eq!(h.state().views[0].page_input, "draft", "re-focusing keeps text already being edited");
}

#[test]
fn page_label_input_new_document_does_not_inherit_or_commit_the_previous_draft() {
    use pdfcraft_engine::{Edit, LabelStyle, Session};
    let mut h = harness();
    restart_page_labels(&mut h);
    h.state_mut().views[0].go_to_page(3);
    h.run_steps(3);
    type_page_input(&mut h, "ii");
    let original_id = h.state().views[0].id;
    let original_bytes = h.state().session.save_bytes(original_id).unwrap();

    let mut source = Session::new().with_clock(|| 1_700_000_000);
    let source_id = source.open("new-labels.pdf", None, std::sync::Arc::new(fixture(5)), None).unwrap();
    for (from, to, style) in [(0, 1, LabelStyle::LowerRoman), (2, 4, LabelStyle::Decimal)] {
        source.apply(source_id, Edit::NumberPages { from, to, style, prefix: String::new(), first: 1 }).unwrap();
    }
    h.state_mut().open_bytes("new-labels.pdf", None, source.save_bytes(source_id).unwrap().as_ref().clone()).unwrap();
    // Enter arriving with the switch must not submit the previous editor into this document.
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    let next = h.state().active.unwrap();
    assert_ne!(h.state().views[next].id, original_id);
    assert_eq!((h.state().views[next].current, h.state().views[next].page_input.as_str()), (0, "i"));
    assert!(!page_input_focused(&h), "the new document's editor starts idle");
    assert_eq!((h.state().views[0].current, h.state().views[0].page_input.as_str()), (3, "ii"), "the hidden valid draft is not committed");
    focus_page_input(&mut h);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(h.state().views[next].current, 0, "unchanged Enter stays on the new document's first page");

    h.get_by_label("a.pdf (edited)").click();
    h.run_steps(3);
    assert_eq!(h.state().active, Some(0));
    assert_eq!((h.state().views[0].current, h.state().views[0].page_input.as_str()), (3, "2"));
    assert_eq!(h.state().session.save_bytes(original_id).unwrap(), original_bytes, "tab switching never edits the original PDF");
}

#[test]
fn page_label_input_preserves_typing_and_refreshes_after_navigation_and_tab_changes() {
    let mut h = harness();
    restart_page_labels(&mut h);
    h.state_mut().views[0].go_to_page(0);
    h.run_steps(3);
    type_page_input(&mut h, "draft");
    h.state_mut().views[0].go_to_page(3);
    h.run_steps(3);
    assert_eq!(h.state().views[0].page_input, "draft", "paging must not overwrite the focused editor");
    let editor_id = h.ctx.memory(|m| m.focused()).expect("focused page editor");
    h.ctx.memory_mut(|m| m.surrender_focus(editor_id));
    h.run_steps(3);
    assert_eq!(h.state().views[0].page_input, "2", "an idle editor follows the current page");

    type_page_input(&mut h, "unsent");
    h.get_by_label("b.pdf").click();
    h.run_steps(3);
    assert_eq!(h.state().active, Some(1));
    assert_eq!(h.state().views[0].page_input, "unsent", "switching tabs keeps the hidden document's draft");
    assert_eq!(h.state().views[1].page_input, "1", "the new document displays its own label");
    h.state_mut().set_option("page", "2").unwrap();
    h.run_steps(3);
    focus_page_input(&mut h);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!((h.state().views[1].current, h.state().views[1].page_input.as_str()), (1, "2"), "default labels still work");
    h.get_by_label("a.pdf (edited)").click();
    h.run_steps(3);
    assert_eq!(h.state().views[0].page_input, "2", "returning to a document refreshes its idle label");

    assert!(h.state_mut().views[0].view_history(false));
    h.run_steps(3);
    assert_eq!(h.state().views[0].page_input, "i", "history navigation refreshes the label");
}

#[test]
fn page_label_input_same_document_replacement_refreshes_the_focused_label() {
    use pdfcraft_engine::{Edit, LabelStyle, Session};
    use std::sync::Arc;
    for (pages, roman, page, label) in [(5, false, 3, "4"), (5, true, 3, "iv"), (3, false, 2, "3")] {
        let mut h = harness();
        h.state_mut().active = Some(0);
        h.state_mut().views[0].go_to_page(3);
        h.run_steps(3);
        assert_eq!((h.state().views[0].current, h.state().views[0].page_input.as_str()), (3, "4"));
        focus_page_input(&mut h);
        let editor_id = h.ctx.memory(|m| m.focused()).expect("focused page editor");
        let id = h.state().views[0].id;
        let before = h.state().session.get(id).unwrap();
        assert!(!before.dirty);
        assert_eq!((before.can_undo(), before.can_redo()), (None, None));

        let replacement = if roman {
            let mut source = Session::new().with_clock(|| 1_700_000_000);
            let source_id = source.open("replacement.pdf", None, Arc::new(fixture(pages)), None).unwrap();
            source
                .apply(source_id, Edit::NumberPages { from: 0, to: pages - 1, style: LabelStyle::LowerRoman, prefix: String::new(), first: 1 })
                .unwrap();
            source.save_bytes(source_id).unwrap()
        } else {
            Arc::new(fixture(pages))
        };
        // The file watcher uses this same reload/replacement path without changing DocId.
        let app = h.state_mut();
        app.session.reload(id, replacement.clone()).unwrap();
        let doc = app.session.get(id).unwrap();
        app.views[0].document_replaced(&doc.info);
        h.run_steps(3);
        assert_eq!(h.state().views[0].id, id);
        assert_eq!(h.ctx.memory(|m| m.focused()), Some(editor_id), "reload retains the actual editor's focus");
        assert!(page_input_focused(&h));
        assert_eq!((h.state().views[0].current, h.state().views[0].page_input.as_str()), (page, label), "replacement labels and page-count clamping");
        h.key_press(egui::Key::Enter);
        h.run_steps(3);
        assert_eq!(
            (h.state().views[0].current, h.state().views[0].page_input.as_str()),
            (page, label),
            "unchanged Enter retains the replacement's physical page"
        );
        assert_eq!(h.state().views[0].id, id);
        let after = h.state().session.get(id).unwrap();
        assert!(!after.dirty, "navigation does not edit the replacement");
        assert_eq!((after.can_undo(), after.can_redo()), (None, None));
        assert_eq!(h.state().session.save_bytes(id).unwrap(), replacement, "navigation preserves replacement PDF bytes");
    }
}

#[test]
fn page_label_input_same_document_replacement_keeps_full_label_selected_for_typing() {
    use pdfcraft_engine::{Edit, LabelStyle, Session};
    use std::sync::Arc;
    for settling_frames in [3, 0] {
        let mut h = harness();
        h.state_mut().active = Some(0);
        h.state_mut().views[0].go_to_page(3);
        h.run_steps(3);
        assert_eq!((h.state().views[0].current, h.state().views[0].page_input.as_str()), (3, "4"));
        assert!(h.state_mut().execute("view.focus_page_input"));
        h.run_steps(3);
        assert!(page_input_focused(&h), "the real focus command focuses the page editor");
        let editor_id = h.ctx.memory(|m| m.focused()).expect("focused page editor");
        let range = egui::TextEdit::load_state(&h.ctx, editor_id).unwrap().cursor.char_range().unwrap();
        let ends = [range.primary.index, range.secondary.index];
        assert!(ends.contains(&egui::text::CCursor::new(0).index), "the old label selection starts at zero");
        assert!(ends.contains(&egui::text::CCursor::new(1).index), "the entire old one-character label is selected");
        let id = h.state().views[0].id;
        let original = h.state().session.get(id).unwrap();
        assert!(!original.dirty);
        assert_eq!((original.can_undo(), original.can_redo()), (None, None));

        let mut source = Session::new().with_clock(|| 1_700_000_000);
        let source_id = source.open("replacement.pdf", None, Arc::new(fixture(5)), None).unwrap();
        source.apply(source_id, Edit::NumberPages { from: 0, to: 4, style: LabelStyle::LowerRoman, prefix: String::new(), first: 1 }).unwrap();
        let replacement = source.save_bytes(source_id).unwrap();
        let app = h.state_mut();
        app.session.reload(id, replacement.clone()).unwrap();
        let doc = app.session.get(id).unwrap();
        app.views[0].document_replaced(&doc.info);
        // Cover the reproduced settled case and typing before the replacement's first frame.
        if settling_frames > 0 {
            h.run_steps(settling_frames);
        }
        assert_eq!(h.state().views[0].id, id);
        assert_eq!(h.ctx.memory(|m| m.focused()), Some(editor_id), "same-DocId reload retains the actual focused editor");
        assert!(page_input_focused(&h));
        assert_eq!((h.state().views[0].current, h.state().views[0].page_input.as_str()), (3, "iv"));

        // Ordinary typing must replace the whole refreshed label without another focus or select-all.
        h.event(egui::Event::Text("2".into()));
        h.run_steps(if settling_frames == 0 { 1 } else { 2 });
        assert!(page_input_focused(&h));
        assert_eq!(h.state().views[0].current, 3, "typing has not submitted navigation yet");
        assert_eq!(h.state().views[0].page_input, "2", "typing replaces the entire replacement label");
        // Selection synchronization is consumed once: later input appends at the caret.
        h.event(egui::Event::Text("x".into()));
        h.run_steps(1);
        assert!(page_input_focused(&h));
        assert_eq!(h.state().views[0].page_input, "2x", "a second character appends without selecting the label again");
        h.key_press(egui::Key::Backspace);
        h.run_steps(1);
        assert_eq!(h.state().views[0].page_input, "2", "Backspace removes only the second character");
        h.key_press(egui::Key::Enter);
        h.run_steps(3);
        assert_eq!((h.state().views[0].current, h.state().views[0].page_input.as_str()), (1, "ii"), "Enter goes to physical page two");
        assert_eq!(h.state().views[0].id, id);
        let after = h.state().session.get(id).unwrap();
        assert!(!after.dirty, "navigation does not edit the replacement PDF");
        assert_eq!((after.can_undo(), after.can_redo()), (None, None));
        assert_eq!(h.state().session.save_bytes(id).unwrap(), replacement, "typing and navigation preserve replacement PDF bytes");
    }
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
        app.set_option("language", "en").unwrap();
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
    // The dialog's height settles a frame after it opens; egui centers from the previous size.
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
    app.set_option("language", "en").unwrap();
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
fn a_placeholder_title_leaves_the_file_name_on_the_tab_and_window() {
    // A browser's "Save as PDF" of a blank page or pop-up titles the document "about:blank"
    // and asks viewers to show the title: the file name says more.
    use pdfcraft_engine::Edit;
    let mut h = harness();
    {
        let s = h.state_mut();
        let id = s.views[s.active.unwrap()].id;
        s.session.apply(id, Edit::SetInfo { key: "Title".into(), value: "about:blank".into() }).unwrap();
        let mut v = s.session.get(id).unwrap().initial_view();
        v.display_title = true;
        s.session.apply(id, Edit::SetInitialView(Box::new(v))).unwrap();
    }
    h.run_steps(2);
    assert_eq!(h.state().window_title, "b.pdf — PdfCraft");
    assert!(h.query_by_label_contains("about:blank").is_none());
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
    // Let the dialog finish centering before the revision button is clicked.
    h.run_steps(2);
    h.get_by_label("View revision 1").click();
    h.run_steps(3);
    let s = h.state();
    assert!(s.dialog.is_none(), "{:?} {:?}", s.dialog, s.session.get(s.views[s.active.unwrap()].id).unwrap().name);
    let doc = s.session.get(s.views[s.active.unwrap()].id).unwrap();
    assert_eq!(doc.name, "updated (revision 1).pdf");
    assert_eq!(doc.info_value("Title"), None, "as it was before the update");
    // A repeat reuses the revision tab instead of opening another identical one.
    let views = s.views.len();
    let ids: Vec<_> = h.state().views.iter().map(|v| v.id).collect();
    let original = {
        let s = h.state();
        ids.iter().position(|id| s.session.get(*id).is_some_and(|d| d.name == "updated.pdf")).unwrap()
    };
    h.state_mut().active = Some(original);
    h.state_mut().open_revision(1);
    h.run_steps(1);
    let s = h.state();
    assert_eq!(s.views.len(), views);
    assert!(
        s.views[s.active.unwrap()]
            .snapshot
            .as_ref()
            .is_some_and(|snap| snap.kind == pdfcraft_ui_egui::canvas::SnapshotKind::Revision && snap.revision == 1)
    );
    // An unknown revision errors without opening a tab.
    h.state_mut().active = Some(original);
    h.state_mut().open_revision(99);
    h.run_steps(1);
    let s = h.state();
    assert_eq!(s.views.len(), views);
    assert!(h.state().toast.as_ref().is_some_and(|t| t.0.starts_with("Couldn't open revision 99")), "unknown revisions error without opening a tab");
    // Another tab over the same original shares the snapshot: b.pdf's first revision is
    // byte-identical to updated.pdf's (incremental saves append), so it reuses the tab.
    let ids: Vec<_> = h.state().views.iter().map(|v| v.id).collect();
    let other = {
        let s = h.state();
        ids.iter().position(|id| s.session.get(*id).is_some_and(|d| d.name == "b.pdf")).unwrap()
    };
    h.state_mut().active = Some(other);
    h.state_mut().open_revision(1);
    h.run_steps(1);
    let s = h.state();
    assert_eq!(s.views.len(), views);
    assert!(
        s.views[s.active.unwrap()]
            .snapshot
            .as_ref()
            .is_some_and(|snap| snap.kind == pdfcraft_ui_egui::canvas::SnapshotKind::Revision && snap.revision == 1)
    );
}

#[test]
fn wrapped_revision_buttons_stay_above_binding_and_open_the_saved_revision() {
    use pdfcraft_engine::{Edit, Session};
    let mut bytes = std::sync::Arc::new(fixture(1));
    // Reopen each save so the next edit appends to the preceding saved revision.
    for revision in 2..=13 {
        let mut source = Session::new().with_clock(|| 1_700_000_000);
        let id = source.open("revisions.pdf", None, bytes, None).unwrap();
        source.apply(id, Edit::SetInfo { key: "Title".into(), value: format!("Revision {revision}") }).unwrap();
        bytes = source.save_bytes(id).unwrap();
    }
    for width in [1400.0, 720.0] {
        let pdf = bytes.clone();
        let mut h = Harness::builder().with_size(egui::vec2(width, 900.0)).build_eframe(move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.open_bytes("revisions.pdf", None, pdf.to_vec()).unwrap();
            app
        });
        let s = h.state();
        let doc = s.session.get(s.views[s.active.unwrap()].id).unwrap();
        assert_eq!(doc.revision_ends().len(), 13, "the fixture contains thirteen saved revisions");
        h.state_mut().dialog = Some(Dialog::Properties(pdfcraft_ui_egui::PropsTab::Advanced));
        // Let the grid and the centered dialog settle before measuring or clicking.
        h.run_steps(6);
        let buttons: Vec<_> = (1..13).map(|n| (n, h.get_by_label(&format!("View revision {n}")).rect())).collect();
        let first_row = h.get_by_label("View revision 12").rect();
        let lower_revision = buttons
            .iter()
            .find(|(n, rect)| *n > 1 && rect.top() > first_row.bottom())
            .map(|(n, _)| *n)
            .expect("revision buttons wrap to a lower row");
        let binding = h.get_by_label("Binding").rect();
        let left = h.get_by_label("Left Edge").rect();
        let right = h.get_by_label("Right Edge").rect();
        // Properties requests a fixed width. Measure its card in each viewport.
        let card = h.ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new("dialog").with("card"))).expect("Properties is shown");
        for (n, rect) in &buttons {
            for (label, below) in [("Binding", binding), ("Left Edge", left), ("Right Edge", right)] {
                assert!(rect.bottom() <= below.top(), "revision {n} overlaps {label} at viewport width {width}: {rect:?}, {below:?}");
            }
        }
        for (n, rect) in &buttons {
            assert!(
                rect.left() >= card.left() && rect.right() <= card.right(),
                "revision {n} escapes Properties horizontally at viewport width {width}: {rect:?}, measured card {card:?}"
            );
        }
        h.get_by_label(&format!("View revision {lower_revision}")).scroll_to_me();
        h.run_steps(3);
        h.get_by_label(&format!("View revision {lower_revision}")).click();
        h.run_steps(3);
        let s = h.state();
        assert!(s.dialog.is_none(), "opening a revision closes Properties");
        let view = &s.views[s.active.unwrap()];
        let doc = s.session.get(view.id).unwrap();
        assert_eq!(doc.name, format!("revisions (revision {lower_revision}).pdf"));
        assert_eq!(doc.info_value("Title"), Some(format!("Revision {lower_revision}")));
        let snapshot = view.snapshot.as_ref().expect("the saved revision opens as a snapshot");
        assert_eq!(snapshot.kind, pdfcraft_ui_egui::canvas::SnapshotKind::Revision);
        assert_eq!(snapshot.revision, lower_revision);
    }
}

#[test]
fn advanced_search_lists_results_with_context() {
    let mut h = harness();
    let i = h.state().active.unwrap();
    assert!(h.state_mut().execute("edit.advanced_search"));
    h.run_steps(2);
    h.get_by_label("What word or phrase would you like to search for?");
    h.state_mut().views[i].find.as_mut().unwrap().query = "pages".into();
    // Background rendering and text extraction can take seconds on a loaded CI runner (FreeBSD VM).
    for _ in 0..3000 {
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

#[test]
fn space_audit_numbers_line_up_under_their_headers() {
    // The percentages were drawn at the dialog's far edge, away from their header and the
    // striped rows: a right-to-left layout in a grid's last column takes the rest of the window.
    let mut h = harness();
    assert!(h.state_mut().execute("optimize.advanced"));
    h.run_steps(2);
    h.get_by_label("Audit space usage…").click();
    h.run_steps(3);
    let total = h.state().space_audit.iter().map(|r| r.bytes).sum::<u64>().to_string();
    let right = |label: &str| h.get_by_label(label).rect().right();
    assert!((right("Percentage") - right("100.00%")).abs() < 1.0, "{} vs {}", right("Percentage"), right("100.00%"));
    assert!((right("Bytes") - right(&total)).abs() < 1.0, "{} vs {}", right("Bytes"), right(&total));
    assert!(right("100.00%") < right("OK") + 1.0, "the table stays inside the dialog");
}

#[test]
fn each_document_keeps_its_own_scroll_position() {
    // #189: every document's page view shared one scroll position, so switching tabs showed a
    // document at wherever the other one had been scrolled to (usually back at the top).
    use pdfcraft_ui_egui::canvas::Fit;
    let mut h = harness();
    for v in &mut h.state_mut().views {
        v.fit = Fit::Width;
    }
    let show = |h: &mut Harness<'static, PdfCraftApp>, i: usize| {
        h.state_mut().active = Some(i);
        h.run_steps(4);
        h.state().views[i].current
    };
    show(&mut h, 0);
    h.state_mut().views[0].goto = Some((3, 0.0));
    h.run_steps(4);
    assert_eq!(h.state().views[0].current, 3, "a.pdf scrolled to page 4");
    // b.pdf opens where it was (the top), and is scrolled to its own last page.
    assert_eq!(show(&mut h, 1), 0);
    h.state_mut().views[1].goto = Some((1, 0.0));
    h.run_steps(4);
    assert_eq!(h.state().views[1].current, 1);
    // Back to a.pdf: still on page 4, and b.pdf still on page 2.
    assert_eq!(show(&mut h, 0), 3);
    assert_eq!(show(&mut h, 1), 1);
}

#[test]
fn a_new_document_opens_at_the_top_after_a_relaunch() {
    // eframe saves egui's memory, scroll offsets included, with the settings and restores it at
    // the next launch, where document ids start again at 1: the first document opened then
    // started at the offset the previous session's first document was left at.
    use pdfcraft_ui_egui::canvas::Fit;
    let mut h = harness();
    for v in &mut h.state_mut().views {
        v.fit = Fit::Width;
    }
    h.state_mut().active = Some(0);
    h.run_steps(4);
    h.state_mut().views[0].goto = Some((3, 0.0));
    h.run_steps(4);
    assert_eq!(h.state().views[0].current, 3, "a.pdf scrolled to page 4");
    // Quit: eframe writes egui's memory to app.ron, as below, and reads it back at the next launch.
    #[derive(Default)]
    struct Settings(std::collections::HashMap<String, String>);
    impl eframe::Storage for Settings {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.into(), value);
        }
        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }
        fn flush(&mut self) {}
    }
    let mut settings = Settings::default();
    h.ctx.memory(|m| eframe::set_value(&mut settings, "egui", m));
    let memory: egui::Memory = eframe::get_value(&settings, "egui").expect("egui's memory round-trips");
    // Next launch: a document never seen before, which gets a.pdf's old id.
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        cc.egui_ctx.memory_mut(|m| *m = memory);
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("c.pdf", None, fixture(5)).unwrap();
        app.views[0].fit = Fit::Width;
        app
    });
    h.run_steps(4);
    assert_eq!(h.state().views[0].id, pdfcraft_engine::DocId(1));
    assert_eq!(h.state().views[0].current, 0, "c.pdf opens on page 1");
    // It is still a scroll position of its own from then on.
    h.state_mut().views[0].goto = Some((2, 0.0));
    h.run_steps(4);
    assert_eq!(h.state().views[0].current, 2);
    h.run_steps(4);
    assert_eq!(h.state().views[0].current, 2, "c.pdf stays on page 3");
}

#[test]
fn arrow_and_page_keys_move_through_a_scrolling_document() {
    // #185: in the continuous (default) and two-page views the arrow keys and Page Down / Up did
    // nothing without ⌘.
    use egui::Key;
    use pdfcraft_ui_egui::canvas::{Fit, PageLayout};
    let mut h = harness();
    h.state_mut().active = Some(0);
    h.state_mut().views[0].fit = Fit::Width;
    h.run_steps(4);
    assert_eq!(h.state().views[0].layout, PageLayout::Single);
    assert!(h.state().views[0].continuous(), "the default view scrolls continuously");
    let press = |h: &mut Harness<'static, PdfCraftApp>, key, times: usize| {
        for _ in 0..times {
            h.key_press(key);
            h.run_steps(2);
        }
        h.run_steps(2);
        h.state().views[0].current
    };
    // → / ← turn pages, as in Acrobat
    assert_eq!(press(&mut h, Key::ArrowRight, 2), 2);
    assert_eq!(press(&mut h, Key::ArrowLeft, 1), 1);
    assert_eq!(press(&mut h, Key::ArrowLeft, 1), 0);
    // ↓ scrolls a line at a time, Page Down a screen: count the presses that reach page 2
    let presses = |h: &mut Harness<'static, PdfCraftApp>, key| {
        let mut n = 0;
        while h.state().views[0].current == 0 {
            press(h, key, 1);
            n += 1;
            assert!(n < 500, "{key:?} never reached page 2");
        }
        n
    };
    let lines = presses(&mut h, Key::ArrowDown);
    assert_eq!(press(&mut h, Key::ArrowUp, lines + 1), 0);
    let screens = presses(&mut h, Key::PageDown);
    assert_eq!(press(&mut h, Key::PageUp, screens + 1), 0);
    assert!(screens >= 2 && lines > 2 * screens, "a fit-width page: {lines} lines, {screens} screens");
    // two-page view: → moves a spread
    h.state_mut().views[0].layout = PageLayout::TwoUp;
    h.run_steps(4);
    assert_eq!(press(&mut h, Key::ArrowRight, 1), 2);
    // single-page view keeps Page Down for the next page
    h.state_mut().views[0].layout = PageLayout::Single;
    h.state_mut().views[0].continuous = false;
    h.run_steps(4);
    assert_eq!(press(&mut h, Key::PageDown, 1), 3);
}

#[test]
fn up_and_down_turn_pages_in_single_page_view() {
    // #273: in single-page view ↓ / ↑ only scrolled, so on a page that fits (or once scrolled to
    // the end) they did nothing, while the wheel turned the page there.
    use egui::Key;
    use pdfcraft_ui_egui::canvas::{Fit, PageLayout};
    let mut h = harness();
    h.state_mut().active = Some(0);
    h.state_mut().views[0].layout = PageLayout::Single;
    h.state_mut().views[0].continuous = false;
    h.state_mut().views[0].fit = Fit::Page;
    h.run_steps(4);
    let press = |h: &mut Harness<'static, PdfCraftApp>, key, times: usize| {
        for _ in 0..times {
            h.key_press(key);
            h.run_steps(2);
        }
        h.run_steps(2);
        h.state().views[0].current
    };
    // A page that fits: each press turns one page.
    assert_eq!(press(&mut h, Key::ArrowDown, 1), 1);
    assert_eq!(press(&mut h, Key::ArrowDown, 1), 2);
    assert_eq!(press(&mut h, Key::ArrowUp, 1), 1);
    assert_eq!(press(&mut h, Key::ArrowUp, 1), 0);
    assert_eq!(press(&mut h, Key::ArrowUp, 1), 0, "nothing before the first page");
    // A page taller than the window scrolls first, then turns at its bottom.
    h.state_mut().views[0].fit = Fit::Width;
    h.run_steps(4);
    let mut lines = 0;
    while press(&mut h, Key::ArrowDown, 1) == 0 {
        lines += 1;
        assert!(lines < 500, "↓ never reached page 2");
    }
    assert!(lines > 2, "↓ scrolled through page 1 before turning ({lines} lines)");
    // ↑ at the top of page 2 goes back to the bottom of page 1, then scrolls up it.
    assert_eq!(press(&mut h, Key::ArrowUp, 1), 0);
    assert_eq!(press(&mut h, Key::ArrowDown, 1), 1, "back at the bottom of page 1");
    assert_eq!(press(&mut h, Key::ArrowUp, 2), 0);
    assert_eq!(press(&mut h, Key::ArrowDown, 1), 0, "scrolled up from the bottom of page 1");
}

#[test]
fn v_h_and_space_pick_the_quick_tools() {
    // The toolbar's tooltips promise "Select (V)" and "Hand (H)", but the keys did nothing.
    use egui::{Key, Modifiers};
    use pdfcraft_ui_egui::QuickTool;
    let mut h = harness();
    h.state_mut().active = Some(0);
    h.run_steps(2);
    let press = |h: &mut Harness<'static, PdfCraftApp>, key| {
        h.key_press(key);
        h.run_steps(2);
        h.state().quick_tool
    };
    assert_eq!(press(&mut h, Key::H), QuickTool::Hand);
    assert_eq!(press(&mut h, Key::V), QuickTool::Select);
    // V leaves any other tool too, as the toolbar button does.
    h.state_mut().quick_tool = QuickTool::Crop;
    assert_eq!(press(&mut h, Key::V), QuickTool::Select);
    // Holding Space pans with the Hand for as long as it is held, then gives the tool back.
    h.state_mut().quick_tool = QuickTool::Crop;
    h.key_down(Key::Space);
    h.run_steps(4);
    assert_eq!(h.state().quick_tool, QuickTool::Hand, "Space held");
    h.key_up(Key::Space);
    h.run_steps(2);
    assert_eq!(h.state().quick_tool, QuickTool::Crop, "Space released");
    // With a modifier the letters are someone else's shortcut.
    h.key_press_modifiers(Modifiers::SHIFT, Key::H);
    h.run_steps(2);
    assert_eq!(h.state().quick_tool, QuickTool::Crop);
    // They are letters to a text field: typing in the find bar keeps the tool.
    h.state_mut().quick_tool = QuickTool::Select;
    h.state_mut().views[0].open_find();
    h.run_steps(4);
    assert_eq!(press(&mut h, Key::H), QuickTool::Select, "typing H in the find bar");
    h.key_down(Key::Space);
    h.run_steps(2);
    h.key_up(Key::Space);
    h.run_steps(2);
    assert_eq!(h.state().quick_tool, QuickTool::Select, "typing a space in the find bar");
}

/// Like [`fixture`], with each page's size given.
fn sized_fixture(sizes: &[(u32, u32)]) -> Vec<u8> {
    let n = sizes.len();
    let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" ")));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
    for (i, (w, h)) in sizes.iter().enumerate() {
        objs.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>",
            5 + 2 * i
        ));
        let body = format!("BT /F1 18 Tf 20 150 Td (Page {}) Tj ET", i + 1);
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

#[test]
fn page_down_steps_every_spread_when_several_fit_on_screen() {
    // #188: in two-page view with a cover page, ⌘Page Down skipped spreads (or stuck) once more
    // than one spread fit on screen: the current page was whichever wholly visible page rounding
    // made "most visible", so the next step started from the wrong spread.
    use egui::{Key, Modifiers};
    use pdfcraft_ui_egui::canvas::{Fit, PageLayout};
    // a printed book: portrait cover and back, landscape two-page spreads between
    let mut sizes = vec![(200, 300)];
    sizes.extend([(400, 300); 10]);
    sizes.push((200, 300));
    for fit in [Fit::Width, Fit::Page, Fit::Height] {
        let bytes = sized_fixture(&sizes);
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.open_bytes("book.pdf", None, bytes).unwrap();
            app
        });
        h.run_steps(6);
        h.state_mut().active = Some(0);
        let v = &mut h.state_mut().views[0];
        (v.fit, v.layout, v.cover) = (fit, PageLayout::TwoUp, true);
        h.run_steps(6);
        let mut step = |key| {
            h.key_press_modifiers(Modifiers::COMMAND, key);
            h.run_steps(4);
            h.state().views[0].current
        };
        let down: Vec<usize> = (0..6).map(|_| step(Key::PageDown)).collect();
        assert_eq!(down, [1, 3, 5, 7, 9, 11], "{fit:?}: cover, five spreads, back cover");
        let up: Vec<usize> = (0..6).map(|_| step(Key::PageUp)).collect();
        assert_eq!(up, [9, 7, 5, 3, 1, 0], "{fit:?}");
    }
}

/// Open `sizes` in Fit width, continuous, with the Pages panel showing.
fn pages_panel_harness(sizes: &[(u32, u32)]) -> Harness<'static, PdfCraftApp> {
    let bytes = sized_fixture(sizes);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("cheques.pdf", None, bytes).unwrap();
        app.right = Some(pdfcraft_ui_egui::RightPanel::Pages);
        app
    });
    h.run_steps(6);
    h.state_mut().active = Some(0);
    h.state_mut().views[0].fit = pdfcraft_ui_egui::canvas::Fit::Width;
    h.run_steps(6);
    h
}

/// A thumbnail in the Pages panel (the panel lists pages as "Page n" buttons).
fn thumbnail(h: &Harness<'static, PdfCraftApp>, label: &str) -> egui::Pos2 {
    h.get_all_by_label(label).find(|n| n.rect().left() > 1000.0).expect("the Pages panel thumbnail").rect().center()
}

/// Press and release `button` at `at`.
fn press(h: &mut Harness<'static, PdfCraftApp>, at: egui::Pos2, button: egui::PointerButton) {
    h.hover_at(at);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: at, button, pressed: true, modifiers: Default::default() });
    h.event(egui::Event::PointerButton { pos: at, button, pressed: false, modifiers: Default::default() });
}

#[test]
fn a_short_page_gone_to_from_the_pages_panel_stays_current() {
    // A short page (a cheque) above a tall one: going to it scrolls it to the top, where the tall
    // page below shows more of itself. The current page, and the Pages panel's highlight, moved
    // to the tall page.
    let mut h = pages_panel_harness(&[(600, 250), (600, 1800), (600, 250), (600, 1800)]);
    for page in [1, 0, 2] {
        let at = thumbnail(&h, &format!("Page {}", page + 1));
        press(&mut h, at, egui::PointerButton::Primary);
        h.run_steps(6);
        assert_eq!(h.state().views[0].current, page, "clicked page {}", page + 1);
    }
}

#[test]
fn pages_panel_thumbnails_have_a_context_menu() {
    let mut h = pages_panel_harness(&[(200, 300); 4]);
    let at = thumbnail(&h, "Page 3");
    press(&mut h, at, egui::PointerButton::Secondary);
    h.run_steps(2);
    // The page right-clicked becomes the selection the menu acts on.
    assert_eq!(h.state().views[0].target_pages(), vec![2]);
    for item in ["Extract pages", "Split document…", "Cut", "Paste after"] {
        h.get_by_label(item);
    }
    h.get_by_label("Copy").click();
    h.run_steps(3);
    assert_eq!(h.state().page_clipboard.as_ref().map(|c| c.pages.clone()), Some(vec![2]));
}

/// Where the Pages panel draws page `n`'s thumbnail.
fn thumbnail_rect(h: &Harness<'static, PdfCraftApp>, n: usize) -> egui::Rect {
    h.get_all_by_label(&format!("Page {n}")).find(|t| t.rect().left() > 1000.0).expect("the Pages panel thumbnail").rect()
}

/// Whether `r` lies wholly in the part of the Pages panel in view.
fn in_panel_view(h: &Harness<'static, PdfCraftApp>, r: egui::Rect) -> bool {
    h.state().views[0].pages_panel.expect("the Pages panel is shown").contains_rect(r)
}

#[test]
fn pages_panel_thumbnail_size_controls_show_more_pages_and_keep_the_selection() {
    let mut h = pages_panel_harness(&[(200, 300); 30]);
    h.state_mut().views[0].select_pages(&[0, 1]);
    h.run_steps(2);
    let usual = thumbnail_rect(&h, 1).width();
    let zoom = h.state().views[0].zoom;
    let visible = |h: &Harness<'static, PdfCraftApp>| (1..=30).filter(|n| in_panel_view(h, thumbnail_rect(h, *n))).count();
    let before = visible(&h);
    h.get_by_label("Smaller thumbnails").click();
    h.run_steps(4);
    assert!(thumbnail_rect(&h, 1).width() < usual);
    assert!(visible(&h) > before, "smaller thumbnails show more complete pages");
    assert_eq!(h.state().views[0].target_pages(), vec![0, 1]);
    assert_eq!(h.state().views[0].current, 0);
    assert_eq!(h.state().views[0].zoom, zoom, "only the thumbnails are resized");
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty);
    if let Ok(dir) = std::env::var("PDFCRAFT_THUMBNAIL_SHOTS") {
        let _gpu = crate::gpu();
        h.render().unwrap().save(format!("{dir}/small.png")).unwrap();
    }
    h.get_by_label("Larger thumbnails").click();
    h.run_steps(4);
    assert!((thumbnail_rect(&h, 1).width() - usual).abs() < 0.1);
    h.get_by_label("Larger thumbnails").click();
    h.run_steps(4);
    assert!(thumbnail_rect(&h, 1).width() > usual);
    assert_eq!(h.state().views[0].target_pages(), vec![0, 1]);
    if let Ok(dir) = std::env::var("PDFCRAFT_THUMBNAIL_SHOTS") {
        let _gpu = crate::gpu();
        h.render().unwrap().save(format!("{dir}/large.png")).unwrap();
        h.state_mut().set_option("theme", "dark").unwrap();
        h.run_steps(4);
        h.render().unwrap().save(format!("{dir}/large-dark.png")).unwrap();
    }
}

#[test]
fn pages_panel_thumbnail_zoom_is_bounded_independent_and_survives_reload() {
    let mut h = pages_panel_harness(&[(200, 300); 6]);
    let usual = thumbnail_rect(&h, 1).width();
    let zoom = h.state().views[0].zoom;
    h.state_mut().set_option("grid-zoom", "200").unwrap();
    h.state_mut().set_option("thumbnail-zoom", "50%").unwrap();
    h.run_steps(4);
    let small = thumbnail_rect(&h, 1).width();
    assert!(small < usual);
    for invalid in ["0", "49", "151", "NaN", "inf", "nope"] {
        assert!(h.state_mut().set_option("thumbnail-zoom", invalid).is_err(), "reject {invalid}");
    }
    h.run_steps(3);
    assert_eq!(thumbnail_rect(&h, 1).width(), small);
    assert_eq!(h.state().views[0].grid_zoom(), 2.0);
    assert_eq!(h.state().views[0].zoom, zoom);
    let info = h.state().session.get(h.state().views[0].id).unwrap().info.clone();
    h.state_mut().views[0].document_replaced(&info);
    h.run_steps(4);
    assert_eq!(thumbnail_rect(&h, 1).width(), small, "file reload keeps the chosen thumbnail size");
    h.state_mut().views[0].set_thumbnail_zoom(f32::NAN);
    assert_eq!(h.state().views[0].thumbnail_zoom(), 0.5);
    h.state_mut().views[0].set_thumbnail_zoom(f32::MAX);
    assert_eq!(h.state().views[0].thumbnail_zoom(), 1.5);
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty);
    let mut empty = PdfCraftApp::new();
    assert!(empty.set_option("thumbnail-zoom", "100").is_err());
}

#[test]
fn pages_panel_thumbnail_zoom_keeps_a_far_current_page_visible() {
    let mut h = pages_panel_harness(&[(200, 300); 30]);
    h.state_mut().views[0].go_to_page(24);
    h.run_steps(4);
    for size in ["50", "150", "80", "100"] {
        h.state_mut().set_option("thumbnail-zoom", size).unwrap();
        h.run_steps(4);
        assert!(in_panel_view(&h, thumbnail_rect(&h, 25)), "page 25 stays visible at {size}%");
        assert_eq!(h.state().views[0].current, 24);
        h.get_by_label("Smaller thumbnails");
        h.get_by_label("Larger thumbnails");
    }
}

/// #2781: as in Acrobat, going to a page whose thumbnail is out of sight scrolls the Pages
/// panel to show it.
#[test]
fn going_to_a_far_page_scrolls_its_thumbnail_into_view() {
    let mut h = pages_panel_harness(&[(200, 300); 30]);
    assert!(!in_panel_view(&h, thumbnail_rect(&h, 25)), "page 25 starts out of sight");
    h.state_mut().views[0].go_to_page(24);
    h.run_steps(4);
    assert_eq!(h.state().views[0].current, 24);
    assert!(in_panel_view(&h, thumbnail_rect(&h, 25)), "page 25's thumbnail is shown");
}

/// #2781: a page whose thumbnail is already in view leaves the panel where it is, rather than
/// centring the thumbnail on every page change.
#[test]
fn going_to_a_page_in_view_leaves_the_pages_panel_still() {
    // Landscape pages, so several thumbnails share the panel.
    let mut h = pages_panel_harness(&[(300, 200); 30]);
    h.state_mut().views[0].go_to_page(14);
    h.run_steps(4);
    let clip = h.state().views[0].pages_panel.expect("the Pages panel is shown");
    // The topmost thumbnail wholly in view, other than the current page's.
    let (n, before) =
        (1..=30).map(|n| (n, thumbnail_rect(&h, n))).find(|(n, r)| *n != 15 && in_panel_view(&h, *r)).expect("another thumbnail in view");
    assert!(
        (before.center().y - clip.center().y).abs() > 50.0,
        "page {n} at {before:?} is away from the middle of {clip:?}, where centring would move it"
    );
    h.state_mut().views[0].go_to_page(n - 1);
    h.run_steps(4);
    assert_eq!(h.state().views[0].current, n - 1);
    assert_eq!(thumbnail_rect(&h, n), before, "page {n}'s thumbnail stayed put");
}

/// #2781: while the user drags thumbnails, a page change does not pull the panel away from
/// where they are being dropped.
#[test]
fn a_page_change_while_thumbnails_are_dragged_does_not_scroll_the_pages_panel() {
    let mut h = pages_panel_harness(&[(200, 300); 30]);
    let first = thumbnail_rect(&h, 1);
    let at = first.center();
    h.hover_at(at);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.run_steps(1);
    h.hover_at(at + egui::vec2(0.0, 30.0));
    h.run_steps(2);
    assert_eq!(h.state().views[0].panel_drag, Some(vec![0]), "page 1 is being dragged");
    h.state_mut().views[0].go_to_page(24);
    h.run_steps(4);
    assert_eq!(h.state().views[0].current, 24);
    assert_eq!(thumbnail_rect(&h, 1), first, "the panel stays put under the drag");
}

/// #444, #186: a mouse-wheel notch scrolls as far as the scroll speed preference says;
/// trackpad (point) scrolling keeps its own speed. The choice is saved.
#[test]
fn the_scroll_speed_preference_scales_mouse_wheel_notches() {
    use pdfcraft_ui_egui::scroll_speed::ScrollSpeed;
    let scrolled = |speed: ScrollSpeed, unit: egui::MouseWheelUnit, dy: f32| -> f32 {
        let mut h = harness();
        h.state_mut().scroll_speed = speed;
        h.run_steps(2);
        let i = h.state().active.expect("a document is shown");
        let top = |h: &Harness<'static, PdfCraftApp>| h.state().views[i].page_screen_rect(0).expect("page 1 on screen").min.y;
        let before = top(&h);
        h.event(egui::Event::PointerMoved(h.state().views[i].viewport_rect().center()));
        h.event(egui::Event::MouseWheel { unit, delta: egui::vec2(0.0, -dy), phase: egui::TouchPhase::Move, modifiers: egui::Modifiers::NONE });
        // egui spreads a wheel notch over a few frames.
        h.run_steps(30);
        before - top(&h)
    };
    let line = egui::MouseWheelUnit::Line;
    let normal = scrolled(ScrollSpeed::Normal, line, 1.0);
    assert!((normal - ScrollSpeed::Normal.line_points()).abs() < 1.0, "one notch at Normal scrolled {normal}");
    let (slow, fast) = (scrolled(ScrollSpeed::Slow, line, 1.0), scrolled(ScrollSpeed::Fast, line, 1.0));
    assert!((slow * 2.0 - normal).abs() < 1.0 && (fast - normal * 2.0).abs() < 1.0, "slow {slow}, normal {normal}, fast {fast}");
    let point = egui::MouseWheelUnit::Point;
    let (normal_points, fast_points) = (scrolled(ScrollSpeed::Normal, point, 30.0), scrolled(ScrollSpeed::Fast, point, 30.0));
    assert!(normal_points > 0.0 && (normal_points - fast_points).abs() < 1.0, "trackpad: {normal_points} vs {fast_points}");
    let mut app = PdfCraftApp::new();
    app.set_option("scroll_speed", "fast").unwrap();
    assert!(app.set_option("scroll_speed", "warp").unwrap_err().contains("slow, normal, or fast"));
    let mut again = PdfCraftApp::new();
    again.restore(&app.persist());
    assert_eq!(again.scroll_speed, ScrollSpeed::Fast);
}

/// A 40 × 30 picture (one red corner) and a harness showing a PDF of it.
fn picture_harness() -> (image::RgbaImage, Harness<'static, PdfCraftApp>) {
    picture_harness_sized(egui::vec2(1400.0, 900.0))
}

/// [`picture_harness`] in a window of this size.
fn picture_harness_sized(size: egui::Vec2) -> (image::RgbaImage, Harness<'static, PdfCraftApp>) {
    let mut px = image::RgbaImage::from_pixel(40, 30, image::Rgba([10, 200, 30, 255]));
    px.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(px.clone()).write_to(&mut png, image::ImageFormat::Png).unwrap();
    let pdf = pdfcraft_engine::Session::new().create_from_images(&[("p.png".into(), png.into_inner())]).unwrap();
    let mut h = Harness::builder().with_size(size).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("pic.pdf", None, pdf.to_vec()).unwrap();
        app
    });
    h.run_steps(4);
    h.state_mut().system_clipboard = false;
    (px, h)
}

#[test]
fn a_clicked_picture_is_selected_and_copied_in_its_original_format() {
    let (px, mut h) = picture_harness();
    let i = h.state().active.unwrap();
    let at = h.state().views[i].page_screen_rect(0).expect("on screen").center();
    h.hover_at(at);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
    assert_eq!(h.state().views[i].picked_image.map(|p| (p.page, p.index)), Some((0, 0)));
    h.event(egui::Event::Copy);
    h.run_steps(2);
    let (name, bytes) = h.state().last_copied_file.clone().expect("the image was copied as a file");
    assert_eq!(name, "pic p1 img1.png");
    let decoded = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).unwrap().into_rgba8();
    assert_eq!(decoded.into_raw(), px.into_raw(), "lossless, at full size");
    // Escape deselects.
    h.key_press(egui::Key::Escape);
    h.run_steps(1);
    assert_eq!(h.state().views[i].picked_image, None);
}

/// A picture picked with the Select tool, then deleted in Edit text & images, leaves no highlight
/// behind on the empty spot where it was.
#[test]
fn a_picked_picture_that_is_deleted_in_edit_mode_loses_its_highlight() {
    let (_, mut h) = picture_harness();
    let i = h.state().active.unwrap();
    let id = h.state().views[i].id;
    let at = h.state().views[i].page_screen_rect(0).expect("on screen").center();
    h.hover_at(at);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
    assert!(h.state().views[i].picked_image.is_some(), "picked with the Select tool");
    // Edit text & images: click the picture, delete it with the Delete key.
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(2);
    h.hover_at(at);
    h.run_steps(1);
    h.drag_at(at);
    h.run_steps(1);
    h.drop_at(at);
    h.run_steps(2);
    h.key_press(egui::Key::Delete);
    // One frame, no more input: the highlight goes at once, it does not wait for a click.
    h.run_steps(1);
    assert!(h.state().session.get(id).unwrap().page_images(0).is_empty(), "the picture is gone");
    assert_eq!(h.state().views[i].picked_image, None, "and so is its highlight");
}

/// The highlight goes in the very edit that removes the picture, not on a later frame: the window
/// only redraws on the next input, so waiting for one leaves the highlight on screen until a click.
#[test]
fn an_edit_clears_the_picked_picture_at_once() {
    let (_, mut h) = picture_harness();
    let i = h.state().active.unwrap();
    let at = h.state().views[i].page_screen_rect(0).expect("on screen").center();
    h.hover_at(at);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
    assert!(h.state().views[i].picked_image.is_some(), "picked with the Select tool");
    let delete = pdfcraft_engine::Edit::EditPageImage { page: 0, index: 0, change: pdfcraft_engine::ImageEdit::Delete };
    assert!(h.state_mut().apply_edit(delete));
    // No frame has run since the edit.
    assert_eq!(h.state().views[i].picked_image, None);
}

#[test]
fn right_click_on_a_picture_copies_it_at_full_size() {
    let (px, mut h) = picture_harness();
    let i = h.state().active.unwrap();
    let at = h.state().views[i].page_screen_rect(0).expect("on screen").center();
    h.hover_at(at);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: true, modifiers: Default::default() });
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
    h.get_by_label("Copy Image").click();
    h.run_steps(2);
    let (w, hgt, rgba) = h.state().last_copied_image.clone().expect("the image was copied");
    assert_eq!((w, hgt), (40, 30));
    assert_eq!(rgba, px.into_raw(), "the image's own pixels, not a rendering of the page");
}

/// A picture drawn through grouped Form artwork with a caption over it (as Word, InDesign and
/// scanners with OCR write them): one click still selects it, and ⌘C copies the picture.
#[test]
fn a_picture_inside_form_artwork_under_text_is_selected_and_copied() {
    let stream = |dict: &str, data: &[u8]| {
        let mut o = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
        o.extend_from_slice(data);
        o.extend_from_slice(b"\nendstream");
        o
    };
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> /XObject << /Fm1 6 0 R >> >> >>".to_vec(),
        stream("", b"q 180 0 0 280 10 10 cm /Fm1 Do Q BT /F1 40 Tf 15 130 Td (WWWWWWW) Tj ET"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 1 1] /Resources << /XObject << /Im1 7 0 R >> >>", b"/Im1 Do"),
        stream("/Type /XObject /Subtype /Image /Width 2 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8", &[255, 0, 0, 0, 0, 255]),
    ];
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        pdf.extend_from_slice(o);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        pdf.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("scan.pdf", None, pdf).unwrap();
        app
    });
    crate::support::settle(&mut h);
    h.state_mut().system_clipboard = false;
    let i = h.state().active.unwrap();
    let r = h.state().views[i].page_screen_rect(0).expect("on screen");
    // On the caption ("WWWWWWW" at y 130–170 of 300, from x 15).
    let at = egui::pos2(r.left() + 60.0 / 200.0 * r.width(), r.top() + (300.0 - 145.0) / 300.0 * r.height());
    h.hover_at(at);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
    assert_eq!(h.state().views[i].picked_image.map(|p| (p.page, p.index)), Some((0, 0)));
    h.event(egui::Event::Copy);
    h.run_steps(2);
    let (name, bytes) = h.state().last_copied_file.clone().expect("the picture was copied");
    assert_eq!(name, "scan p1 img1.png");
    let png = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).unwrap().into_rgba8();
    assert_eq!(png.into_raw(), vec![255, 0, 0, 255, 0, 0, 255, 255]);
}

/// A picture in Edit text & images, right-clicked so its menu is open (in a tall window, so the
/// whole menu is on screen).
fn picture_menu() -> (image::RgbaImage, Harness<'static, PdfCraftApp>) {
    let (px, mut h) = picture_harness_sized(egui::vec2(1400.0, 1500.0));
    h.state_mut().set_option("quick", "edit-text").unwrap();
    h.run_steps(3);
    let i = h.state().active.unwrap();
    let at = h.state().views[i].page_screen_rect(0).expect("on screen").center();
    h.hover_at(at);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: true, modifiers: Default::default() });
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: false, modifiers: Default::default() });
    h.run_steps(3);
    (px, h)
}

fn picture_count(h: &Harness<'static, PdfCraftApp>) -> usize {
    let s = h.state();
    s.session.get(s.active_ids().unwrap().1).unwrap().page_images(0).len()
}

#[test]
fn the_picture_menu_cuts_a_picture_to_the_clipboard_and_removes_it() {
    let (px, mut h) = picture_menu();
    assert_eq!(picture_count(&h), 1);
    h.get_by_label("Cut Image").click();
    h.run_steps(3);
    let (w, hgt, rgba) = h.state().last_copied_image.clone().expect("the image was copied first");
    assert_eq!((w, hgt), (40, 30));
    assert_eq!(rgba, px.into_raw());
    assert_eq!(picture_count(&h), 0, "then taken off the page");
    h.state_mut().undo();
    h.run_steps(2);
    assert_eq!(picture_count(&h), 1, "one undo step puts it back");
}

#[test]
fn the_picture_menu_marks_a_picture_for_redaction() {
    let (_, mut h) = picture_menu();
    let marks = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        s.session.get(s.active_ids().unwrap().1).unwrap().redaction_marks()
    };
    assert_eq!(marks(&h), 0);
    h.get_by_label("Redact Image").click();
    h.run_steps(3);
    assert_eq!(marks(&h), 1);
    assert_eq!(picture_count(&h), 1, "marked, not yet removed: Redact a PDF applies it");
}

#[test]
fn the_picture_menu_opens_link_properties_and_recognize_text_for_the_picture() {
    let (_, mut h) = picture_menu();
    h.get_by_label("Create Link…").click();
    h.run_steps(3);
    assert!(matches!(h.state().dialog, Some(pdfcraft_ui_egui::Dialog::LinkProps)));
    let draft = h.state().link_draft.clone().expect("the link draft");
    assert_eq!(draft.page, 0);
    assert!(draft.rect[2] > draft.rect[0] && draft.rect[3] > draft.rect[1], "over the picture: {:?}", draft.rect);

    let (_, mut h) = picture_menu();
    h.get_by_label("Recognize Text…").click();
    h.run_steps(3);
    assert!(matches!(h.state().dialog, Some(pdfcraft_ui_egui::Dialog::RecognizeText)));
    let d = &h.state().ocr_draft;
    assert_eq!((format!("{:?}", d.pages), d.from, d.to), ("Range".to_string(), 1, 1));
}

/// The menu is longer than the picture is tall, so most items lie outside it: the pointer moving
/// onto them must not close the menu (it used to be drawn only while the pointer was over the
/// picture, so Delete and the items below the page could not be clicked).
#[test]
fn every_item_of_the_picture_menu_can_be_clicked_outside_the_picture() {
    let (_, mut h) = picture_menu();
    let s = h.state();
    let page = s.views[s.active.unwrap()].page_screen_rect(0).expect("on screen");
    let delete = h.get_by_label("Delete").rect();
    assert!(delete.center().y > page.bottom(), "Delete lies below the page: {delete:?} vs {page:?}");
    assert_eq!(picture_count(&h), 1);
    h.get_by_label("Delete").click();
    h.run_steps(3);
    assert_eq!(picture_count(&h), 0);
}

#[test]
fn mouse_wheel_scrolls_while_selecting_text() {
    let mut h = harness();
    h.state_mut().set_option("zoom", "200").unwrap();
    h.state_mut().set_option("left", "closed").unwrap();
    h.state_mut().set_option("panel", "none").unwrap();
    h.run_steps(8);
    let view = h.state().active.unwrap();
    let page = h.state().views[view].page_screen_rect(0).unwrap();
    let start = egui::pos2(page.left() + page.width() * 0.11, page.top() + page.height() * 0.48);
    let end = start + egui::vec2(100.0, 0.0);
    h.hover_at(start);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: start, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.run_steps(1);
    h.hover_at(end);
    h.run_steps(2);
    assert!(h.state().views[view].selected_text().is_some_and(|s| !s.is_empty()), "a text selection is active");
    assert!(h.ctx.dragged_id().is_some(), "the primary drag is still held");
    let before = h.state().views[view].page_screen_rect(0).unwrap().top();
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: egui::vec2(0.0, -3.0),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(20);
    let after = h.state().views[view].page_screen_rect(0).unwrap().top();
    assert!(after < before - 1.0, "wheel should move the page during selection: {before} -> {after}");
    assert!(h.state().views[view].selected_text().is_some_and(|s| !s.is_empty()));
    h.event(egui::Event::PointerButton { pos: end, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    h.run_steps(2);
}
fn selection_wheel_harness() -> (Harness<'static, PdfCraftApp>, usize, egui::Pos2) {
    let mut h = harness();
    h.state_mut().set_option("zoom", "200").unwrap();
    h.state_mut().set_option("left", "closed").unwrap();
    h.state_mut().set_option("panel", "none").unwrap();
    h.run_steps(8);
    let view = h.state().active.unwrap();
    let page = h.state().views[view].page_screen_rect(0).unwrap();
    let start = egui::pos2(page.left() + page.width() * 0.11, page.top() + page.height() * 0.48);
    let end = start + egui::vec2(100.0, 0.0);
    h.hover_at(start);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: start, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.run_steps(1);
    h.hover_at(end);
    h.run_steps(2);
    assert!(h.state().views[view].selected_text().is_some_and(|s| !s.is_empty()), "a text selection is active");
    assert!(h.ctx.dragged_id().is_some(), "the primary drag is still held");

    (h, view, end)
}

fn selection_wheel_delta(unit: egui::MouseWheelUnit, amount: f32, held: bool, animated: bool) -> f32 {
    let (mut h, view, end) = selection_wheel_harness();
    h.state_mut().animation_effects =
        if animated { pdfcraft_ui_egui::motion::AnimationEffects::On } else { pdfcraft_ui_egui::motion::AnimationEffects::Off };
    if !held {
        h.event(egui::Event::PointerButton { pos: end, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    }
    h.run_steps(2);
    let before = h.state().views[view].page_screen_rect(0).unwrap().top();
    h.event(egui::Event::MouseWheel { unit, delta: egui::vec2(0.0, -amount), phase: egui::TouchPhase::Move, modifiers: egui::Modifiers::NONE });
    h.run_steps(40);
    before - h.state().views[view].page_screen_rect(0).unwrap().top()
}

#[test]
fn selection_wheel_preserves_normal_distance_and_reduced_motion() {
    for animated in [true, false] {
        let normal = selection_wheel_delta(egui::MouseWheelUnit::Line, 1.0, false, animated);
        let held = selection_wheel_delta(egui::MouseWheelUnit::Line, 1.0, true, animated);
        assert!(normal > 1.0);
        assert!((normal - held).abs() < 1.0, "animated={animated}: released={normal}, held={held}");
    }
    let on = selection_wheel_delta(egui::MouseWheelUnit::Line, 1.0, true, true);
    let off = selection_wheel_delta(egui::MouseWheelUnit::Line, 1.0, true, false);
    assert!((on - off).abs() < 1.0, "on={on}, off={off}");
}

#[test]
fn selection_point_scroll_is_not_applied_twice() {
    for animated in [true, false] {
        let normal = selection_wheel_delta(egui::MouseWheelUnit::Point, 30.0, false, animated);
        let held = selection_wheel_delta(egui::MouseWheelUnit::Point, 30.0, true, animated);
        eprintln!("Point animated={animated}: released={normal}, held={held}");
        assert!(normal > 1.0, "point wheel must scroll: {normal}");
        assert!((held - 30.0).abs() < 1.0, "animated={animated}: held precision motion={held}, normal={normal}");
    }
}

#[test]
fn selection_control_wheel_still_zooms() {
    for modifiers in [egui::Modifiers::CTRL, egui::Modifiers::COMMAND] {
        let (mut h, view, _) = selection_wheel_harness();
        let before = h.state().views[view].page_screen_rect(0).unwrap().width();
        h.event(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta: egui::vec2(0.0, 1.0), phase: egui::TouchPhase::Move, modifiers });
        h.run_steps(40);
        let after = h.state().views[view].page_screen_rect(0).unwrap().width();
        assert!(after > before + 1.0, "control wheel should zoom: {before} -> {after}");
    }
}

#[test]
fn selection_wheel_outside_document_does_not_scroll() {
    let (mut h, view, _) = selection_wheel_harness();
    h.hover_at(egui::pos2(1398.0, 2.0));
    h.run_steps(2);
    let before = h.state().views[view].page_screen_rect(0).unwrap().top();
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: egui::vec2(0.0, -1.0),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(40);
    let after = h.state().views[view].page_screen_rect(0).unwrap().top();
    assert!((after - before).abs() < 1.0, "outside wheel moved document: {before} -> {after}");
}

#[test]
fn selection_page_wheel_preserves_normal_distance() {
    for animated in [true, false] {
        let normal = selection_wheel_delta(egui::MouseWheelUnit::Page, 0.1, false, animated);
        let held = selection_wheel_delta(egui::MouseWheelUnit::Page, 0.1, true, animated);
        assert!(normal > 1.0);
        assert!((normal - held).abs() < 1.0, "animated={animated}: released={normal}, held={held}");
    }
}

#[test]
fn selection_wheel_does_not_scroll_behind_preferences() {
    let (mut h, view, _) = selection_wheel_harness();
    h.state_mut().set_option("dialog", "preferences").unwrap();
    h.run_steps(3);
    let before = h.state().views[view].page_screen_rect(0).unwrap().top();
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: egui::vec2(0.0, -1.0),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(40);
    let after = h.state().views[view].page_screen_rect(0).unwrap().top();
    assert!((after - before).abs() < 1.0, "modal wheel moved document: {before} -> {after}");
}

#[test]
fn selection_mouse_wheel_resumes_after_point_scrolling() {
    for delay in [1, 16, 40] {
        let (mut h, view, _) = selection_wheel_harness();
        let start = h.state().views[view].page_screen_rect(0).unwrap().top();
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -30.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
        h.run_steps(delay);
        let before = h.state().views[view].page_screen_rect(0).unwrap().top();
        // One Line notch follows this platform's configured speed. A Point tail can
        // still add travel in the one-frame case, so require at least the whole notch.
        let expected = h.ctx.options(|o| o.input_options.line_scroll_speed);
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, -1.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
        h.run_steps(40);
        let after = h.state().views[view].page_screen_rect(0).unwrap().top();
        eprintln!("Point-to-Line delay={delay}: point={}, line={}", start - before, before - after);
        let travelled = before - after;
        assert!(travelled >= expected - 1.0, "Line input must resume after Point at the configured speed: {travelled} vs {expected}");
        if delay >= 16 {
            assert!((travelled - expected).abs() < 1.0, "settled Line travel must match its configured speed: {travelled} vs {expected}");
        }
    }
}

#[test]
fn page_command_without_selection_acts_on_the_page_pressed() {
    let mut h = harness(); // b.pdf (2 pages) is active
    h.state_mut().set_option("zoom", "50").unwrap();
    h.run_steps(3);
    // Page 2 was gone to earlier; both pages show whole, so it stays current.
    h.state_mut().set_option("page", "2").unwrap();
    h.run_steps(5);
    let at = |h: &Harness<'static, PdfCraftApp>, page: usize| {
        let v = &h.state().views[h.state().active.unwrap()];
        v.page_screen_rect(page).expect("page on screen").center()
    };
    let first = at(&h, 0);
    h.event(egui::Event::PointerMoved(first));
    h.run_steps(2);
    h.event(egui::Event::PointerButton { pos: first, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.run_steps(2);
    h.event(egui::Event::PointerButton { pos: first, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    h.run_steps(3);
    assert_eq!(h.state().views[h.state().active.unwrap()].current, 0, "the pressed page is the current one");
    h.state_mut().execute("page.rotate");
    h.run_steps(3);
    let s = h.state();
    let d = s.session.get(s.views[s.active.unwrap()].id).unwrap();
    assert_eq!(d.info.pages.iter().map(|p| p.rotation).collect::<Vec<_>>(), [90, 0]);
}

/// ↓ / ↑ follow the Scroll speed preference as the wheel does (#985): Fast reaches the next page
/// in about half the presses of Normal, and Slow takes about twice as many.
#[test]
fn arrow_keys_follow_the_scroll_speed_preference() {
    use egui::Key;
    let lines_to_page_2 = |speed: &str| {
        let mut h = harness();
        h.state_mut().active = Some(0);
        h.state_mut().set_option("scroll_speed", speed).unwrap();
        h.run_steps(4);
        let mut n = 0;
        while h.state().views[0].current == 0 {
            h.key_press(Key::ArrowDown);
            h.run_steps(2);
            n += 1;
            assert!(n < 1000, "↓ never reached page 2 at {speed}");
        }
        n
    };
    let (slow, normal, fast) = (lines_to_page_2("slow"), lines_to_page_2("normal"), lines_to_page_2("fast"));
    assert!(fast * 3 < normal * 2 && normal * 3 < slow * 2, "presses to page 2: slow {slow}, normal {normal}, fast {fast}");
}

/// #2665: next to a very wide page the normal pages sit far to its right, outside the window.
/// Going to one changed the page counter but kept the horizontal scroll, so it stayed out of
/// sight; as in Acrobat, it is brought into view.
#[test]
fn go_to_page_brings_the_page_into_view_horizontally() {
    use pdfcraft_ui_egui::canvas::Fit;
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("wide.pdf", None, sized_fixture(&[(595, 842), (595, 842), (595, 842), (6000, 2000)])).unwrap();
        app.views[0].fit = Fit::None;
        app.views[0].zoom = 1.0;
        app
    });
    h.run_steps(4);
    assert!(h.state().views[0].page_screen_rect(0).is_none(), "page 1 starts off-screen, right of the window");
    h.state_mut().views[0].go_to_page(2);
    h.run_steps(4);
    let v = &h.state().views[0];
    assert_eq!(v.current, 2);
    let (r, vp) = (v.page_screen_rect(2).expect("page 3 on screen"), v.viewport_rect());
    assert!(r.left() >= vp.left() && r.right() <= vp.right(), "page 3 at {r:?}, viewport {vp:?}");
}
