//! Headless UI tests for editing and saving: the organize toolbar, selection, undo/redo keys,
//! save/save-as, the unsaved-changes prompt and editable document properties.

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_render::{PageRenderer, RenderRequest, RequestKind};
use pdfcraft_ui_egui::{CloseRequest, PdfCraftApp};

/// An `n`-page document with a proper xref table; page `i` shows "Page i+1".
fn fixture(n: usize) -> Vec<u8> {
    let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 200 300] >>", kids.join(" ")));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
    for i in 0..n {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * i));
        let body = format!("BT /F1 24 Tf 20 150 Td (Page {}) Tj ET", i + 1);
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
    }
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

fn harness(pages: usize, setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("doc.pdf", None, fixture(pages)).expect("fixture opens");
        setup(&mut app);
        app
    });
    h.run_steps(4);
    h
}

fn organize(pages: usize) -> Harness<'static, PdfCraftApp> {
    harness(pages, |app| app.set_option("organize", "on").unwrap())
}

/// Page labels of the active document, read back from its current bytes.
fn page_texts(app: &PdfCraftApp) -> Vec<String> {
    let doc = app.session.get(app.views[0].id).unwrap();
    let mut r = PageRenderer::new(doc.bytes.clone(), Default::default());
    (0..r.page_count())
        .map(|p| {
            let out = r.render(RenderRequest { page: p, kind: RequestKind::Text, scale: 1.0, ..Default::default() });
            out.text.map(|t| t.plain_text().trim().to_string()).unwrap_or_default()
        })
        .collect()
}

fn dirty(h: &Harness<'static, PdfCraftApp>) -> bool {
    let app = h.state();
    app.session.get(app.views[0].id).is_some_and(|d| d.dirty)
}

fn temp_path(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("pdfcraft-ui-tests-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

#[test]
fn organize_delete_then_undo_and_redo_with_keys() {
    let mut h = organize(3);
    h.get_by_label("Page 2").click();
    h.run_steps(2);
    h.get_by_label_contains("1 page selected");
    h.get_by_label("Delete pages (Delete)").click();
    h.run_steps(3);
    assert_eq!(page_texts(h.state()), ["Page 1", "Page 3"]);
    assert!(dirty(&h));
    h.get_by_label("doc.pdf (edited)"); // the tab shows unsaved changes

    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert_eq!(page_texts(h.state()), ["Page 1", "Page 2", "Page 3"]);
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    h.run_steps(3);
    assert_eq!(page_texts(h.state()), ["Page 1", "Page 3"]);
}

#[test]
fn shift_click_selects_a_range_and_moves_it() {
    let mut h = organize(4);
    h.get_by_label("Page 1").click();
    h.run_steps(1);
    h.get_by_label("Page 2").click_modifiers(Modifiers::SHIFT);
    h.run_steps(2);
    h.get_by_label_contains("2 pages selected");
    h.get_by_label("Move later").click();
    h.run_steps(3);
    assert_eq!(page_texts(h.state()), ["Page 3", "Page 1", "Page 2", "Page 4"]);
    // The moved pages stay selected at their new position, so repeated clicks keep moving them.
    assert_eq!(h.state().views[0].selected.iter().copied().collect::<Vec<_>>(), [1, 2]);
    h.get_by_label("Move later").click();
    h.run_steps(3);
    assert_eq!(page_texts(h.state()), ["Page 3", "Page 4", "Page 1", "Page 2"]);
}

#[test]
fn command_click_toggles_and_rotation_applies_to_selection() {
    let mut h = organize(3);
    h.get_by_label("Page 1").click();
    h.run_steps(1);
    h.get_by_label("Page 3").click_modifiers(Modifiers::COMMAND);
    h.run_steps(2);
    h.get_by_label("Rotate clockwise").click();
    h.run_steps(3);
    let rot: Vec<u16> = {
        let app = h.state();
        app.session.get(app.views[0].id).unwrap().info.pages.iter().map(|p| p.rotation).collect()
    };
    assert_eq!(rot, [90, 0, 90]);
}

#[test]
fn organize_select_all_shortcut_preserves_current_page_and_sets_range_anchor() {
    let mut h = organize(4);
    h.get_by_label("Page 3").click();
    h.run_steps(1);
    // Automation may move the current selection without a click's range anchor.
    h.state_mut().views[0].select_pages(&[1]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(2);
    h.get_by_label_contains("4 pages selected");
    assert_eq!(h.state().views[0].target_pages(), [0, 1, 2, 3]);
    assert_eq!(h.state().views[0].current, 1, "selecting all keeps the reader's place");
    assert!(!dirty(&h), "selection doesn't edit the document");

    // Repeating Select all is harmless; deleting all pages still keeps the document intact.
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(1);
    h.key_press(Key::Delete);
    h.run_steps(2);
    assert_eq!(page_texts(h.state()).len(), 4);
    assert!(!dirty(&h));
    h.get_by_label("Page 4").click_modifiers(Modifiers::SHIFT);
    h.run_steps(2);
    assert_eq!(h.state().views[0].target_pages(), [1, 2, 3], "Shift-click extends from the current page");
    h.get_by_label("Page 2").click_modifiers(Modifiers::COMMAND);
    h.run_steps(2);
    assert_eq!(h.state().views[0].target_pages(), [2, 3], "command-click still toggles a page");
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(h.state().views[0].selected.is_empty());
}

#[test]
fn organize_select_all_applies_operations_to_every_page() {
    let mut h = organize(3);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(2);
    assert_eq!(h.state().views[0].target_pages(), [0, 1, 2]);
    h.get_by_label("Rotate clockwise").click();
    h.run_steps(3);
    let app = h.state();
    let rotations: Vec<_> = app.session.get(app.views[0].id).unwrap().info.pages.iter().map(|p| p.rotation).collect();
    assert_eq!(rotations, [90, 90, 90]);
}

#[test]
fn organize_select_all_works_without_page_editing_permission() {
    let mut h = harness(3, |app| {
        app.apply_edit(pdfcraft_engine::Edit::Protect(pdfcraft_engine::Protection {
            permissions_password: Some("owner".into()),
            changes: pdfcraft_engine::Changes::None,
            ..Default::default()
        }));
        let bytes = app.session.save_bytes(app.views[0].id).unwrap();
        app.open_bytes("restricted.pdf", None, bytes.as_ref().clone()).unwrap();
        app.set_option("organize", "on").unwrap();
    });
    let index = h.state().active.unwrap();
    assert!(!h.state().session.get(h.state().views[index].id).unwrap().allows_assembly());
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(2);
    assert_eq!(h.state().views[index].target_pages(), [0, 1, 2]);
    assert!(!h.state().session.get(h.state().views[index].id).unwrap().dirty);

    let mut h = organize(1);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(2);
    assert_eq!(h.state().views[0].target_pages(), [0]);
    assert_eq!(h.state().views[0].selected.len(), 1);

    // The opener refuses zero-page PDFs, but the view method also handles empty geometry.
    let mut empty = pdfcraft_ui_egui::canvas::DocView::new(pdfcraft_engine::DocId(0), &Default::default());
    empty.organize = true;
    assert!(!empty.select_all());
    assert!(empty.selected.is_empty());
}

#[test]
fn organize_select_all_leaves_focused_text_input_and_dialogs_alone() {
    let mut h = organize(3);
    h.query_all_by_value("1").next().expect("current page input").focus();
    h.run_steps(1);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(1);
    h.query_all_by_value("1").next().expect("current page input").type_text("2");
    h.run_steps(2);
    assert_eq!(h.state().views[0].page_input, "2", "Ctrl/Cmd+A selected the field's text");
    assert!(h.state().views[0].selected.is_empty());
    h.key_press(Key::Enter);
    h.run_steps(2);
    assert_eq!(h.state().views[0].current, 1);

    h.state_mut().execute("help.about");
    h.run_steps(3);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(2);
    assert!(h.state().views[0].selected.is_empty(), "a modal dialog isolates the page selection");
    h.state_mut().dialog = None;
    h.state_mut().execute("view.palette");
    h.run_steps(3);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(2);
    assert!(h.state().views[0].selected.is_empty(), "the palette keeps its own keyboard input");
}

#[test]
fn delete_is_refused_when_it_would_remove_every_page() {
    let mut h = organize(2);
    h.state_mut().views[0].select_pages(&[0, 1]);
    h.run_steps(2);
    h.key_press(Key::Delete);
    h.run_steps(3);
    assert_eq!(page_texts(h.state()).len(), 2, "a document keeps at least one page");
    assert!(!dirty(&h));
}

#[test]
fn insert_blank_page_after_selection() {
    let mut h = organize(2);
    h.get_by_label("Page 1").click();
    h.run_steps(1);
    h.get_by_label("Insert a blank page after the selection").click();
    h.run_steps(3);
    assert_eq!(page_texts(h.state()), ["Page 1", "", "Page 2"]);
    let app = h.state();
    let info = &app.session.get(app.views[0].id).unwrap().info;
    assert_eq!((info.pages[1].width, info.pages[1].height), (200.0, 300.0), "matches the neighbouring page");
}

#[test]
fn save_writes_an_incremental_update_and_clears_dirty() {
    let out = temp_path("saved.pdf");
    let original = fixture(3);
    let target = out.to_string_lossy().into_owned();
    let mut h = organize(3);
    h.state_mut().save_override = Some(target.clone());
    h.get_by_label("Page 3").click();
    h.run_steps(1);
    h.get_by_label("Rotate counterclockwise").click();
    h.run_steps(3);
    h.key_press_modifiers(Modifiers::COMMAND, Key::S);
    h.run_steps(3);
    let saved = std::fs::read(&out).expect("file written");
    assert_eq!(&saved[..original.len()], &original[..], "original bytes untouched");
    assert!(!dirty(&h));
    h.get_by_label("saved.pdf"); // the tab takes the new name, no edited marker
    // What was written is what the app now shows.
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().bytes.as_slice(), saved.as_slice());
    let reopened = pdfcraft_render::inspect(std::sync::Arc::new(saved), None).unwrap();
    assert_eq!(reopened.pages[2].rotation, 270);
    let _ = std::fs::remove_file(out);
}

#[test]
fn closing_a_dirty_tab_asks_and_cancel_keeps_it() {
    let mut h = organize(2);
    h.get_by_label("Rotate clockwise").click();
    h.run_steps(3);
    h.key_press_modifiers(Modifiers::COMMAND, Key::W);
    h.run_steps(3);
    h.get_by_label_contains("Save changes to “doc.pdf”");
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert_eq!(h.state().views.len(), 1, "cancel keeps the document open");
    assert!(dirty(&h));
    assert!(h.state().close_request.is_none());

    h.key_press_modifiers(Modifiers::COMMAND, Key::W);
    h.run_steps(3);
    h.get_by_label("Don't save").click();
    h.run_steps(3);
    assert!(h.state().views.is_empty(), "discarding closes the tab");
}

#[test]
fn closing_a_dirty_tab_can_save_first() {
    let out = temp_path("closed.pdf");
    let mut h = organize(2);
    h.state_mut().save_override = Some(out.to_string_lossy().into_owned());
    h.get_by_label("Rotate clockwise").click();
    h.run_steps(3);
    h.state_mut().request_close_tab(0);
    h.run_steps(2);
    h.get_by_label("Save").click();
    h.run_steps(3);
    assert!(h.state().views.is_empty());
    let saved = pdfcraft_render::inspect(std::sync::Arc::new(std::fs::read(&out).unwrap()), None).unwrap();
    assert_eq!(saved.pages[0].rotation, 90);
    let _ = std::fs::remove_file(out);
}

#[test]
fn the_save_prompt_answers_to_the_keyboard() {
    // Issue #8: Enter saves; ⌘D / Ctrl+D, Alt+D and Alt+N don't save; Escape cancels. While the
    // prompt is open, ⌘D is not Document properties.
    for (m, key) in [(Modifiers::COMMAND, Key::D), (Modifiers::ALT, Key::D), (Modifiers::ALT, Key::N)] {
        let mut h = organize(2);
        h.get_by_label("Rotate clockwise").click();
        h.run_steps(3);
        h.key_press_modifiers(Modifiers::COMMAND, Key::W);
        h.run_steps(3);
        h.get_by_label_contains("Save changes to “doc.pdf”");
        h.key_press_modifiers(m, key);
        h.run_steps(3);
        assert!(h.state().views.is_empty(), "{m:?}+{key:?} doesn't save and closes");
        assert!(h.state().dialog.is_none(), "{m:?}+{key:?} ran no command underneath");
    }
    let mut h = organize(2);
    h.get_by_label("Rotate clockwise").click();
    h.run_steps(3);
    h.key_press_modifiers(Modifiers::COMMAND, Key::W);
    h.run_steps(3);
    h.key_press(Key::Escape);
    h.run_steps(3);
    assert_eq!(h.state().views.len(), 1, "Escape cancels");
    assert!(h.state().close_request.is_none());

    let out = temp_path("enter-saves.pdf");
    h.state_mut().save_override = Some(out.to_string_lossy().into_owned());
    h.key_press_modifiers(Modifiers::COMMAND, Key::W);
    h.run_steps(3);
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert!(h.state().views.is_empty(), "Enter saves and closes");
    let saved = pdfcraft_render::inspect(std::sync::Arc::new(std::fs::read(&out).unwrap()), None).unwrap();
    assert_eq!(saved.pages[0].rotation, 90);
    let _ = std::fs::remove_file(out);
}

#[test]
fn clean_tabs_close_without_asking() {
    let mut h = harness(1, |_| {});
    h.key_press_modifiers(Modifiers::COMMAND, Key::W);
    h.run_steps(3);
    assert!(h.state().views.is_empty());
    assert!(h.query_by_label("Don't save").is_none());
}

#[test]
fn quitting_with_unsaved_changes_asks_for_each_document() {
    let mut h = harness(2, |app| {
        app.open_bytes("second.pdf", None, fixture(1)).unwrap();
    });
    // Edit both documents.
    for tab in 0..2 {
        h.state_mut().active = Some(tab);
        h.state_mut().views[tab].select_pages(&[0]);
        h.state_mut().apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
    }
    h.state_mut().close_request = Some(CloseRequest::Quit);
    h.run_steps(3);
    h.get_by_label_contains("Save changes to “doc.pdf”");
    h.get_by_label("Don't save").click();
    h.run_steps(3);
    h.get_by_label_contains("Save changes to “second.pdf”");
    h.get_by_label("Don't save").click();
    h.run_steps(3);
    assert!(h.state().views.is_empty());
    assert!(h.state().close_request.is_none());
}

#[test]
fn save_prompt_stays_inside_the_screen_for_a_long_filename() {
    // Issue #161: an unwrapped title carrying a long filename widened the centered modal past
    // the viewport, clipping the message and pushing the Save/Cancel buttons off-screen.
    let name = "Psychology_ The Science of Mind and Behaviour, -- Nigel Holt, Andy Bremner, Michael \
                Vliek, Ed Sutherland, -- 5, 2024 -- McGraw-Hill Education (UK) Ltd -- isbn13 97815268.pdf";
    let mut h = Harness::builder().with_size(egui::vec2(1365.0, 719.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes(name, None, fixture(1)).expect("fixture opens");
        app.close_request = Some(CloseRequest::Tab(0));
        app
    });
    h.run_steps(4);
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1365.0, 719.0));
    let inside = |r: egui::Rect| screen.contains(r.min) && screen.contains(r.max);
    let title = h.get_by_label_contains("Save changes to");
    let title_rect = title.rect();
    assert!(inside(title_rect), "the title rect {title_rect:?} leaves the screen");
    for button in ["Save", "Cancel", "Don't save"] {
        let rect = h.get_by_label(button).rect();
        assert!(inside(rect), "the {button} button rect {rect:?} leaves the screen");
    }
}

#[test]
fn document_properties_edit_is_one_undoable_step() {
    let mut h = harness(1, |app| app.set_option("dialog", "properties").unwrap());
    let title = h.get_by_role_and_label(Role::TextInput, "Title");
    title.focus();
    title.type_text("Annual report");
    h.run_steps(2);
    let author = h.get_by_role_and_label(Role::TextInput, "Author");
    author.focus();
    author.type_text("Finance team");
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
    let app = h.state();
    let doc = app.session.get(app.views[0].id).unwrap();
    assert_eq!(doc.info.title.as_deref(), Some("Annual report"), "inspection sees the new title");
    assert_eq!(doc.info_value("Author").as_deref(), Some("Finance team"));
    assert_eq!(doc.can_undo(), Some("Change document properties"));
    assert!(app.dialog.is_none());

    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    let app = h.state();
    let doc = app.session.get(app.views[0].id).unwrap();
    assert_eq!(doc.info.title, None);
    assert_eq!(doc.info_value("Author"), None);
}

#[test]
fn cancelling_properties_discards_the_draft() {
    let mut h = harness(1, |app| app.set_option("dialog", "properties").unwrap());
    let title = h.get_by_role_and_label(Role::TextInput, "Title");
    title.focus();
    title.type_text("Draft");
    h.run_steps(2);
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert!(!dirty(&h));
    assert!(h.state().props_draft.is_none());
}

#[test]
fn edit_menu_names_the_step_to_undo() {
    let mut h = harness(2, |app| {
        app.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
    });
    h.get_by_label("Menu").click();
    h.run_steps(2);
    h.get_by_label("Edit ⏵").hover(); // submenus open on hover; their labels carry the arrow
    h.run_steps(3);
    h.get_by_label_contains("Undo Rotate page").click(); // the label includes the shortcut
    h.run_steps(3);
    let app = h.state();
    let doc = app.session.get(app.views[0].id).unwrap();
    assert_eq!(doc.can_redo(), Some("Rotate page"));
    assert_eq!(doc.info.pages[0].rotation, 0);
}

// ── Combine / insert from file / extract / split ──────────────────────────────────────────────

fn texts_of(app: &PdfCraftApp, tab: usize) -> Vec<String> {
    let doc = app.session.get(app.views[tab].id).unwrap();
    let mut r = PageRenderer::new(doc.bytes.clone(), Default::default());
    (0..r.page_count())
        .map(|p| {
            let out = r.render(RenderRequest { page: p, kind: RequestKind::Text, scale: 1.0, ..Default::default() });
            out.text.map(|t| t.plain_text().trim().to_string()).unwrap_or_default()
        })
        .collect()
}

#[test]
fn combining_files_opens_a_new_unsaved_tab() {
    let mut h = harness(1, |app| {
        app.use_files(pdfcraft_ui_egui::FilePurpose::Combine, vec![("one.pdf".into(), fixture(2)), ("two.pdf".into(), fixture(1))]);
    });
    h.run_steps(3);
    h.get_by_label("Combine").click();
    h.run_steps(3);
    let app = h.state();
    assert_eq!(app.views.len(), 2);
    assert_eq!(app.active, Some(1));
    assert_eq!(texts_of(app, 1), ["Page 1", "Page 2", "Page 1"]);
    let doc = app.session.get(app.views[1].id).unwrap();
    assert_eq!(doc.name, "Combined.pdf");
    assert!(doc.dirty && doc.path.is_none(), "unsaved until the user saves it");
    assert_eq!(doc.info.outline.iter().map(|o| o.title.as_str()).collect::<Vec<_>>(), ["one", "two"]);
    h.get_by_label("Combined.pdf (edited)");
}

#[test]
fn combine_files_takes_chosen_pages_in_the_order_listed() {
    let mut h = harness(1, |app| {
        app.use_files(pdfcraft_ui_egui::FilePurpose::Combine, vec![("one.pdf".into(), fixture(3)), ("two.pdf".into(), fixture(2))]);
    });
    h.run_steps(3);
    h.get_by_label_contains("Files are combined in this order");
    h.get_by_label("3 pages");
    // two.pdf first; one.pdf's pages 3 and 1.
    h.get_all_by_label("Move up").last().unwrap().click();
    h.run_steps(2);
    h.state_mut().combine_draft[1].range = "3, 1".into();
    h.run_steps(1);
    h.get_by_label("Combine").click();
    h.run_steps(3);
    let app = h.state();
    assert_eq!(texts_of(app, 1), ["Page 1", "Page 2", "Page 3", "Page 1"]);
    let doc = app.session.get(app.views[1].id).unwrap();
    assert_eq!(doc.info.outline.iter().map(|o| o.title.as_str()).collect::<Vec<_>>(), ["two", "one"]);
    assert!(app.combine_draft.is_empty());
}

#[test]
fn extract_button_copies_selected_pages_to_a_new_tab() {
    let mut h = organize(3);
    h.get_by_label("Page 2").click();
    h.run_steps(1);
    h.get_by_label("Page 3").click_modifiers(Modifiers::COMMAND);
    h.run_steps(2);
    h.get_by_label("Extract pages to a new document").click();
    h.run_steps(3);
    h.get_by_label("2 pages selected.");
    h.get_all_by_label("Extract").last().unwrap().click();
    h.run_steps(3);
    let app = h.state();
    assert_eq!(app.views.len(), 2);
    assert_eq!(texts_of(app, 1), ["Page 2", "Page 3"]);
    assert_eq!(texts_of(app, 0).len(), 3, "the original is unchanged");
    assert!(!app.session.get(app.views[0].id).unwrap().dirty);
}

#[test]
fn inserting_a_file_goes_after_the_selection_and_undoes() {
    let mut h = organize(2);
    h.get_by_label("Page 1").click();
    h.run_steps(2);
    h.state_mut().use_files(pdfcraft_ui_egui::FilePurpose::InsertPages, vec![("extra.pdf".into(), fixture(2))]);
    h.run_steps(3);
    assert_eq!(texts_of(h.state(), 0), ["Page 1", "Page 1", "Page 2", "Page 2"]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert_eq!(texts_of(h.state(), 0), ["Page 1", "Page 2"]);
}

#[test]
fn split_dialog_writes_one_file_per_part() {
    let dir = temp_path("split-count");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let d = dir.to_string_lossy().into_owned();
    let mut h = organize(3);
    h.state_mut().export_dir_override = Some(d);
    h.get_by_label("Split into files…").click();
    h.run_steps(3);
    h.get_by_label_contains("Creates 3 files from 3 pages");
    h.get_by_label("Split").click();
    h.run_steps(3);
    let mut names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, ["doc (page 1).pdf", "doc (page 2).pdf", "doc (page 3).pdf"]);
    for name in names {
        let info = pdfcraft_render::inspect(std::sync::Arc::new(std::fs::read(dir.join(name)).unwrap()), None).unwrap();
        assert_eq!(info.pages.len(), 1);
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn split_before_selected_pages() {
    let dir = temp_path("split-sel");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = organize(4);
    h.state_mut().export_dir_override = Some(dir.to_string_lossy().into_owned());
    h.state_mut().views[0].select_pages(&[2]);
    h.state_mut().split_draft.mode = pdfcraft_ui_egui::SplitMode::Selection;
    h.state_mut().run_command("page.split");
    h.run_steps(3);
    h.get_by_label_contains("Creates 2 files from 4 pages");
    h.get_by_label("Split").click();
    h.run_steps(3);
    let mut names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, ["doc (pages 1-2).pdf", "doc (pages 3-4).pdf"]);
    let _ = std::fs::remove_dir_all(dir);
}

// ── Encrypted documents ───────────────────────────────────────────────────────────────────────

fn protected(user: &str, owner: &str, permissions: i32) -> Vec<u8> {
    let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(fixture(2))).unwrap();
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: user,
        owner_password: owner,
        permissions,
        encrypt_metadata: true,
        seed: [4; 32],
    })
    .unwrap();
    pdfcraft_cos::write_full(&doc, &Default::default()).unwrap()
}

#[test]
fn password_prompt_opens_and_security_tab_reports_the_details() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("secret.pdf", None, protected("pw", "owner", -1)).unwrap();
        app
    });
    h.run_steps(4);
    h.get_by_label_contains("is protected");
    let field = h.get_by_role(Role::PasswordInput);
    field.focus();
    field.type_text("pw");
    h.run_steps(2);
    h.key_press(Key::Enter);
    h.run_steps(4);
    assert_eq!(h.state().views.len(), 1);
    h.state_mut().set_option("dialog", "properties").unwrap();
    h.run_steps(2);
    h.get_by_label("Security").click();
    h.run_steps(3);
    h.get_by_label("AES, 256-bit");
    h.get_by_label("User password");
}

#[test]
fn restricted_documents_show_a_notice_and_block_page_changes() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("locked.pdf", None, protected("", "owner", 0b0100)).unwrap(); // opens without a password
        app.set_option("organize", "on").unwrap();
        app
    });
    h.run_steps(4);
    h.get_by_label_contains("This document is secured");
    h.get_by_label("Page 1").click();
    h.run_steps(1);
    h.get_by_label("Delete pages (Delete)").click();
    h.key_press(Key::Delete);
    h.run_steps(3);
    assert_eq!(texts_of(h.state(), 0).len(), 2, "page changes are blocked");
    assert!(!dirty(&h));
    h.get_by_label("Security settings").click();
    h.run_steps(3);
    assert!(h.query_all_by_label("Not allowed").count() >= 4);
}

#[test]
fn replace_pages_dialog_swaps_page_content() {
    let mut app = PdfCraftApp::new();
    app.open_bytes("doc.pdf", None, fixture(3)).unwrap();
    app.views[0].select_pages(&[1]);
    app.start_replace("other.pdf".into(), fixture(5));
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| app);
    h.run_steps(3);
    egui_kittest::kittest::Queryable::get_by_label(&h, "Replace Pages");
    h.state_mut().replace_draft.as_mut().unwrap().src_from = 5;
    h.run_steps(1);
    egui_kittest::kittest::Queryable::get_by_label(&h, "OK").click();
    h.run_steps(3);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(doc.info.pages.len(), 3);
    assert_eq!(doc.can_undo(), Some("Replace page"));
}

#[test]
fn extract_options_and_rotate_pages_dialog() {
    let dir = temp_path("extract-sep");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = organize(4);
    h.state_mut().export_dir_override = Some(dir.to_string_lossy().into_owned());
    h.state_mut().views[0].select_pages(&[1, 2]);
    h.state_mut().run_command("page.extract");
    h.run_steps(2);
    h.state_mut().extract_draft = pdfcraft_ui_egui::ExtractDraft { separate: true, delete: true };
    h.get_all_by_label("Extract").last().unwrap().click();
    h.run_steps(3);
    let mut names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, ["doc (page 2).pdf", "doc (page 3).pdf"]);
    assert_eq!(texts_of(h.state(), 0), ["Page 1", "Page 4"], "deleted after extracting");
    let _ = std::fs::remove_dir_all(dir);
    // Rotate Pages: odd page numbers only.
    h.state_mut().run_command("page.rotate_dialog");
    h.run_steps(2);
    h.state_mut().rotate_draft.which = 0;
    h.state_mut().rotate_draft.parity = pdfcraft_engine::PageParity::Odd;
    h.get_by_label("OK").click();
    h.run_steps(3);
    let s = h.state();
    let d = s.session.get(s.views[0].id).unwrap();
    assert_eq!(d.info.pages.iter().map(|p| p.rotation).collect::<Vec<_>>(), [90, 0]);
}

#[test]
fn dragging_thumbnails_reorders_pages() {
    let mut h = organize(4);
    let before = page_texts(h.state());
    let grab = |h: &Harness<'static, PdfCraftApp>, label: &str| h.get_by_label(label).rect();
    let (from, to) = (grab(&h, "Page 1").center(), grab(&h, "Page 3").right_center() - egui::vec2(10.0, 0.0));
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    for k in 1..=5 {
        h.hover_at(from + (to - from) * (k as f32 / 5.0));
        h.run_steps(1);
    }
    h.drop_at(to);
    h.run_steps(4);
    // Page 1 now sits after page 3.
    let after = page_texts(h.state());
    assert_eq!(after, vec![before[1].clone(), before[2].clone(), before[0].clone(), before[3].clone()], "{after:?}");
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Move page"));
    let sel: Vec<usize> = h.state().views[0].selected.iter().copied().collect();
    assert_eq!(sel, vec![2], "the moved page stays selected");
}

#[test]
fn copying_cutting_and_pasting_pages() {
    let mut h = organize(4);
    let before = page_texts(h.state());
    h.state_mut().views[0].select_pages(&[0]);
    assert!(h.state_mut().execute("page.copy"));
    h.state_mut().views[0].select_pages(&[2]);
    assert!(h.state_mut().execute("page.paste"));
    h.run_steps(2);
    let after = page_texts(h.state());
    assert_eq!(after, vec![before[0].clone(), before[1].clone(), before[2].clone(), before[0].clone(), before[3].clone()], "pasted after page 3");
    let sel: Vec<usize> = h.state().views[0].selected.iter().copied().collect();
    assert_eq!(sel, vec![3], "the pasted page is selected");
    // Cut page 2 and paste it at the end.
    h.state_mut().views[0].select_pages(&[1]);
    assert!(h.state_mut().execute("page.cut"));
    h.state_mut().views[0].select_pages(&[3]);
    assert!(h.state_mut().execute("page.paste"));
    let after = page_texts(h.state());
    assert_eq!(after, vec![before[0].clone(), before[2].clone(), before[0].clone(), before[3].clone(), before[1].clone()]);
    // Every page can be copied but not cut.
    h.state_mut().views[0].select_pages(&[0, 1, 2, 3, 4]);
    h.state_mut().execute("page.cut");
    assert_eq!(page_texts(h.state()).len(), 5);
}

fn source_font_fixture() -> Vec<u8> {
    let body = "BT /F1 18 Tf 20 220 Td (Serif) Tj ET BT /F2 18 Tf 20 120 Td (Mono) Tj ET";
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 300 300] >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Times-BoldItalic >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 7 0 R /Resources << /Font << /F1 3 0 R /F2 5 0 R >> >> >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Courier-Oblique >>".into(),
        "<< /Producer (PdfCraft) >>".into(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
    ];
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

fn open_source_font_fixture() -> Harness<'static, PdfCraftApp> {
    Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("fonts.pdf", None, source_font_fixture()).expect("font fixture opens");
        app
    })
}

#[test]
fn clicking_existing_text_selects_its_source_font_style() {
    let mut h = open_source_font_fixture();
    h.run_steps(4);
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(2);
    let page = h.state().views[0].page_screen_rect(0).expect("on screen");
    let click = |x: f32, y: f32| egui::pos2(page.left() + x / 300.0 * page.width(), page.top() + (300.0 - y) / 300.0 * page.height());

    let serif = click(25.0, 228.0);
    h.hover_at(serif);
    h.run_steps(1);
    h.drag_at(serif);
    h.run_steps(1);
    h.drop_at(serif);
    h.run_steps(3);
    let ed = h.state().views[0].line_editor.clone().expect("serif editor opens");
    assert_eq!(ed.look.family, pdfcraft_engine::FontFamily::Times);
    assert!(ed.look.bold && ed.look.italic, "source style: {:?}", ed.look);

    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    let mono = click(25.0, 128.0);
    h.hover_at(mono);
    h.run_steps(1);
    h.drag_at(mono);
    h.run_steps(1);
    h.drop_at(mono);
    h.run_steps(3);
    let ed = h.state().views[0].line_editor.clone().expect("mono editor opens");
    assert_eq!(ed.look.family, pdfcraft_engine::FontFamily::Courier);
    assert!(!ed.look.bold && ed.look.italic, "source style: {:?}", ed.look);
}

#[test]
fn editing_existing_text_in_place() {
    let mut h = harness(1, |_| {});
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(2);
    // The line "Page 1" sits at (20, 150) on a 200 × 300 page, 24 pt.
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let at = egui::pos2(r.left() + 40.0 / 200.0 * r.width(), r.top() + (300.0 - 158.0) / 300.0 * r.height());
    h.hover_at(at);
    h.run_steps(1);
    h.drag_at(at);
    h.run_steps(1);
    h.drop_at(at);
    h.run_steps(3);
    let ed = h.state().views[0].line_editor.clone().expect("the line opens for editing");
    assert_eq!((ed.page, ed.block, ed.text.as_str()), (0, 0, "Page 1"));
    // A single-line paragraph's box may grow to the page's edge as the text does.
    assert!(ed.growth().is_some(), "the editor box may grow");
    h.state_mut().views[0].line_editor.as_mut().unwrap().text = "Chapter One".into();
    h.run_steps(1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Enter);
    h.run_steps(4);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(doc.can_undo(), Some("Edit text"));
    assert_eq!(doc.text_lines(0)[0].text, "Chapter One");
    assert_eq!(texts_of(s, 0), ["Chapter One"], "the page shows it");
}

/// One page whose only line is drawn twice at the same spot (fake bold, as many generated
/// documents do).
fn double_drawn() -> Vec<u8> {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 200 300] >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> >> >>".into(),
        format!(
            "<< /Length {} >>\nstream\nBT /F1 24 Tf 20 150 Td (Page 1) Tj ET BT /F1 24 Tf 20 150 Td (Page 1) Tj ET\nendstream",
            "BT /F1 24 Tf 20 150 Td (Page 1) Tj ET BT /F1 24 Tf 20 150 Td (Page 1) Tj ET".len()
        ),
    ];
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

#[test]
fn editing_a_double_drawn_line_replaces_every_copy() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("bold.pdf", None, double_drawn()).expect("opens");
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(2);
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let at = egui::pos2(r.left() + 40.0 / 200.0 * r.width(), r.top() + (300.0 - 158.0) / 300.0 * r.height());
    h.hover_at(at);
    h.run_steps(1);
    h.drag_at(at);
    h.run_steps(1);
    h.drop_at(at);
    h.run_steps(3);
    let ed = h.state().views[0].line_editor.clone().expect("the line opens for editing");
    assert_eq!(ed.text, "Page 1");
    h.state_mut().views[0].line_editor.as_mut().unwrap().text = "Replaced".into();
    h.run_steps(1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Enter);
    h.run_steps(4);
    let s = h.state();
    assert_eq!(s.session.get(s.views[0].id).unwrap().text_lines(0).len(), 1, "one line");
    assert_eq!(texts_of(s, 0), ["Replaced"], "no copy of the old text shows under it");
}

#[test]
fn editing_existing_images_on_the_page() {
    // A page made from a 40 × 20 image (at 72 dpi: a 40 × 20 pt page filled by it).
    let mut png = Vec::new();
    image::RgbImage::from_pixel(80, 40, image::Rgb([200, 40, 40])).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("picture.png", None, png.clone()).expect("opens");
        app
    });
    h.run_steps(4);
    let id = h.state().views[0].id;
    let before = h.state().session.get(id).unwrap().page_images(0)[0].rect;
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(2);
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    // Select it, then drag it a quarter of the page to the right.
    let c = r.center();
    h.hover_at(c);
    h.run_steps(1);
    h.drag_at(c);
    h.run_steps(1);
    h.drop_at(c);
    h.run_steps(2);
    assert!(h.state().views[0].image_selection.is_some(), "selected");
    let to = c + egui::vec2(r.width() / 4.0, 0.0);
    h.hover_at(c);
    h.run_steps(1);
    h.drag_at(c);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(c + (to - c) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    h.drop_at(to);
    h.run_steps(4);
    let doc = h.state().session.get(id).unwrap();
    assert_eq!(doc.can_undo(), Some("Move image"));
    let moved = doc.page_images(0)[0].rect;
    let dx = moved[0] - before[0];
    assert!((dx - (before[2] - before[0]) / 4.0).abs() < 1.5, "moved {dx} pt: {before:?} → {moved:?}");
    // Delete with the Delete key.
    h.key_press(egui::Key::Delete);
    h.run_steps(4);
    assert!(h.state().session.get(id).unwrap().page_images(0).is_empty());
}

#[test]
fn the_format_panel_restyles_the_paragraph_being_edited() {
    let mut h = harness(1, |_| {});
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(2);
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let at = egui::pos2(r.left() + 40.0 / 200.0 * r.width(), r.top() + (300.0 - 158.0) / 300.0 * r.height());
    h.hover_at(at);
    h.run_steps(1);
    h.drag_at(at);
    h.run_steps(1);
    h.drop_at(at);
    h.run_steps(3);
    assert!(h.state().views[0].line_editor.is_some());
    // The Format text panel shows the paragraph's look; making it bold applies at once.
    h.get_by_label("FORMAT TEXT");
    h.get_by_label("B").click();
    h.run_steps(4);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(doc.can_undo(), Some("Edit text"));
    assert_eq!(doc.text_blocks(0)[0].base_font, "Helvetica-Bold");
    assert!(s.views[0].line_editor.is_some(), "still editing");
    // Underline: another step, the text keeps its bold.
    h.get_by_label("Underline").click();
    h.run_steps(4);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(doc.text_blocks(0)[0].base_font, "Helvetica-Bold");
    assert!(s.views[0].line_editor.as_ref().is_some_and(|e| e.extras.underline));
}

/// Drag with the pointer from `from` to `to` in a few steps.
fn drag(h: &mut Harness<'static, PdfCraftApp>, from: egui::Pos2, to: egui::Pos2) {
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    for k in 1..=5 {
        h.hover_at(from + (to - from) * (k as f32 / 5.0));
        h.run_steps(1);
    }
    h.drop_at(to);
    h.run_steps(4);
}

#[test]
fn dragging_a_paragraph_moves_it_and_its_edge_rewraps_it() {
    let mut h = harness(1, |_| {});
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(2);
    let block = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().text_blocks(0)[0].clone()
    };
    // User space (200 × 300 page) → screen.
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let k = r.width() / 200.0;
    let screen = |x: f64, y: f64| egui::pos2(r.left() + x as f32 * k, r.top() + (300.0 - y as f32) * k);
    // Move "Page 1" 30 pt right and 50 pt up.
    let before = block(&h);
    let mid = screen((before.rect[0] + before.rect[2]) / 2.0, (before.rect[1] + before.rect[3]) / 2.0);
    drag(&mut h, mid, mid + egui::vec2(30.0 * k, -50.0 * k));
    assert!(h.state().views[0].line_editor.is_none(), "dragging moves the box; it doesn't open it for typing");
    let moved = block(&h);
    assert_eq!(moved.text, "Page 1");
    let near = |a: f64, b: f64| (a - b).abs() < 1.5;
    assert!(near(moved.rect[0], before.rect[0] + 30.0) && near(moved.rect[1], before.rect[1] + 50.0), "{:?} → {:?}", before.rect, moved.rect);
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Edit text"));
    // Drag the handle on its right edge in to about 45 pt wide: "Page" and "1" rewrap onto two lines.
    let edge = screen(moved.rect[2], (moved.rect[1] + moved.rect[3]) / 2.0) + egui::vec2(2.0, 0.0);
    let narrower = (moved.rect[2] - moved.rect[0] - 45.0) as f32 * k;
    drag(&mut h, edge, edge - egui::vec2(narrower, 0.0));
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    let lines: Vec<String> = doc.text_lines(0).iter().map(|l| l.text.clone()).collect();
    assert_eq!(lines, ["Page", "1"], "rewrapped to the narrower box");
    assert!(near(doc.text_lines(0)[0].rect[0], moved.rect[0]), "it keeps its place");
    assert!(h.state().views[0].line_editor.is_none());
}
