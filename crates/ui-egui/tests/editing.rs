//! Headless UI tests for editing and saving: the organize toolbar, selection, undo/redo keys,
//! save/save-as, the unsaved-changes prompt and editable document properties.

mod support;

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use pdfcraft_render::{PageRenderer, RenderRequest, RequestKind};
use pdfcraft_ui_egui::{CloseRequest, Dialog, PdfCraftApp};

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
    let mut builder = Harness::builder().with_size(egui::vec2(1400.0, 900.0));
    if std::env::var_os("PDFCRAFT_ORGANIZE_SNAPSHOT").is_some() {
        builder = builder.wgpu();
    }
    let mut h = builder.build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
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

/// Organize pages ▸ Insert ▸ `item`.
fn insert_menu(h: &mut Harness<'static, PdfCraftApp>, item: &str) {
    h.get_by_label("Insert").click();
    h.run_steps(2);
    h.get_by_label(item).click();
}

fn type_selection(h: &mut Harness<'static, PdfCraftApp>, text: &str) {
    use egui::accesskit::Role;
    h.get_by_role_and_label(Role::TextInput, "Selected pages").click();
    h.run_steps(1);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.get_by_role_and_label(Role::TextInput, "Selected pages").type_text(text);
    h.run_steps(2);
}

#[test]
fn organize_page_range_tracks_thumbnails_and_applies_typed_selection() {
    use egui::accesskit::Role;
    let mut h = organize(100);
    h.get_by_label("Select page 2").click();
    h.run_steps(2);
    assert_eq!(h.get_by_role_and_label(Role::TextInput, "Selected pages").value().as_deref(), Some("2"));
    h.get_by_label("Select page 3").click();
    h.run_steps(2);
    assert_eq!(h.get_by_role_and_label(Role::TextInput, "Selected pages").value().as_deref(), Some("2-3"));
    let old = h.state().views[0].selected.clone();
    type_selection(&mut h, "1,12,16-18,89,92,93,97,99");
    assert_eq!(h.state().views[0].selected, old, "typing is not committed until Enter or focus loss");
    h.key_press(Key::Enter);
    h.run_steps(2);
    assert_eq!(h.state().views[0].target_pages(), [0, 11, 15, 16, 17, 88, 91, 92, 96, 98]);
    assert_eq!(h.get_by_role_and_label(Role::TextInput, "Selected pages").value().as_deref(), Some("1, 12, 16-18, 89, 92-93, 97, 99"));
    if let Some(path) = std::env::var_os("PDFCRAFT_ORGANIZE_SNAPSHOT") {
        h.render().unwrap().save(std::path::Path::new(&path)).unwrap();
    }
    h.get_by_label("Rotate clockwise").click();
    h.run_steps(3);
    let doc = h.state().session.get(h.state().views[0].id).unwrap();
    for (i, page) in doc.info.pages.iter().enumerate() {
        assert_eq!(page.rotation, if h.state().views[0].selected.contains(&i) { 90 } else { 0 });
    }
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert!(h.state().session.get(h.state().views[0].id).unwrap().info.pages.iter().all(|p| p.rotation == 0));
}

#[test]
fn organize_page_range_rejects_invalid_input_and_escape_cancels_without_clearing() {
    use egui::accesskit::Role;
    let mut h = organize(5);
    h.state_mut().set_option("select", "2-3").unwrap();
    h.run_steps(2);
    for text in ["0", "6", "1,", "no"] {
        type_selection(&mut h, text);
        h.key_press(Key::Enter);
        h.run_steps(2);
        assert_eq!(h.state().views[0].target_pages(), [1, 2]);
    }
    type_selection(&mut h, "1-5");
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert_eq!(h.state().views[0].target_pages(), [1, 2]);
    assert_eq!(h.get_by_role_and_label(Role::TextInput, "Selected pages").value().as_deref(), Some("2-3"));
    type_selection(&mut h, "5, 4, 4");
    h.key_press(Key::Enter);
    h.run_steps(2);
    assert_eq!(h.state().views[0].target_pages(), [3, 4]);
    type_selection(&mut h, " ");
    h.key_press(Key::Enter);
    h.run_steps(2);
    assert!(h.state().views[0].selected.is_empty());
    assert!(h.state_mut().set_option("select", "0").is_err());
    assert!(h.state_mut().set_option("select", "6").is_err());
}

#[test]
fn organize_tool_opens_the_overview_directly_and_can_be_reentered() {
    let mut h = harness(12, |_| {});
    assert!(!h.state().views[0].organize);
    h.get_by_label("Organize pages").click();
    h.run_steps(3);
    assert!(h.state().views[0].organize);
    h.get_by_label("Page 1");
    h.get_by_label("Page 5");
    h.get_by_label("Select page 2").click();
    h.run_steps(2);
    h.get_by_label("Close").click();
    h.run_steps(2);
    assert!(!h.state().views[0].organize);
    h.state_mut().set_option("tool", "organize").unwrap();
    h.state_mut().set_option("tool", "organize").unwrap();
    h.run_steps(2);
    assert!(h.state().views[0].organize, "entering the tool is idempotent");
    assert_eq!(h.state().views[0].target_pages(), [1]);
    h.state_mut().set_option("tool", "edit").unwrap();
    h.run_steps(2);
    assert!(!h.state().views[0].organize, "another tool returns to the document");
}

/// Issue #1910: the page grid opens on the page the user was reading: it is selected and
/// scrolled into view, whether the document was on a middle page, its last page or a page
/// other than an earlier grid selection.
#[test]
fn organize_opens_on_the_current_page() {
    let mut h = harness(60, |_| {});
    let in_view = |h: &Harness<'static, PdfCraftApp>, page: usize| {
        let viewport = h.state().views[0].viewport_rect();
        let label = format!("Page {}", page + 1);
        h.query_by_label(&label).is_some_and(|n| viewport.contains_rect(n.rect()))
    };
    // From a middle page.
    h.state_mut().views[0].go_to_page(30);
    h.run_steps(2);
    assert!(h.state_mut().execute("page.organize"));
    h.run_steps(4);
    assert!(h.state().views[0].organize);
    assert_eq!(h.state().views[0].target_pages(), [30]);
    assert!(h.state().views[0].selected.is_empty(), "an empty selection is the current page, so checkboxes still toggle");
    assert!(in_view(&h, 30), "the current page is scrolled into view");
    assert!(!in_view(&h, 0), "the grid does not open on page 1");
    // An earlier grid selection on another page does not win over the page the user went to.
    h.get_by_label("Select page 32").click();
    h.run_steps(2);
    assert_eq!(h.state().views[0].target_pages(), [31]);
    h.get_by_label("Close").click();
    h.run_steps(2);
    assert!(!h.state().views[0].organize);
    h.state_mut().views[0].go_to_page(45);
    h.run_steps(2);
    h.state_mut().set_option("tool", "organize").unwrap();
    h.run_steps(4);
    assert_eq!(h.state().views[0].target_pages(), [45]);
    assert!(in_view(&h, 45));
    assert!(!in_view(&h, 1));
    // From the last page.
    h.get_by_label("Close").click();
    h.run_steps(2);
    h.state_mut().views[0].go_to_page(59);
    h.run_steps(2);
    h.state_mut().set_option("organize", "on").unwrap();
    h.run_steps(4);
    assert_eq!(h.state().views[0].target_pages(), [59]);
    assert!(in_view(&h, 59));
    assert!(!in_view(&h, 0));
}

#[test]
fn organize_tool_chosen_before_opening_a_document_shows_its_pages() {
    let mut app = PdfCraftApp::new();
    app.set_option("tool", "organize").unwrap();
    app.open_bytes("doc.pdf", None, fixture(3)).unwrap();
    assert!(app.views[0].organize);
}

#[test]
fn organize_checkboxes_toggle_multiple_pages_without_modifiers() {
    let mut h = organize(4);
    h.get_by_label("Select page 1").click();
    h.run_steps(2);
    h.get_by_label("Select page 3").click();
    h.run_steps(2);
    assert_eq!(h.state().views[0].target_pages(), [0, 2]);
    h.get_by_label("Select page 1").click();
    h.run_steps(2);
    assert_eq!(h.state().views[0].target_pages(), [2]);
    h.get_by_label("Page 4").click_modifiers(Modifiers::SHIFT);
    h.run_steps(2);
    assert_eq!(h.state().views[0].target_pages(), [0, 1, 2, 3], "checkbox toggles update the range anchor");
}

#[test]
fn organize_page_actions_rotate_only_the_named_page_and_undo() {
    let mut h = organize(3);
    h.get_by_label("Select page 1").click();
    h.run_steps(2);
    h.get_by_label("Select page 3").click();
    h.run_steps(2);
    h.get_by_label("Rotate page 1 clockwise").click();
    h.run_steps(3);
    let rotations = |h: &Harness<'static, PdfCraftApp>| {
        h.state().session.get(h.state().views[0].id).unwrap().info.pages.iter().map(|p| p.rotation).collect::<Vec<_>>()
    };
    assert_eq!(rotations(&h), [90, 0, 0]);
    assert_eq!(h.state().views[0].target_pages(), [0, 2], "a local action preserves the multi-selection");
    h.get_by_label("Rotate counterclockwise").click();
    h.run_steps(3);
    assert_eq!(rotations(&h), [0, 0, 270], "toolbar actions apply to the selection");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert_eq!(rotations(&h), [90, 0, 0]);
    h.get_by_label("Rotate page 1 counterclockwise").click();
    h.run_steps(3);
    assert_eq!(rotations(&h), [0, 0, 0]);
}

#[test]
fn organize_hover_actions_delete_only_the_named_page_and_undo() {
    let mut h = organize(3);
    h.get_by_label("Page 2").hover();
    h.run_steps(2);
    h.get_by_label("Delete page 2").click();
    h.run_steps(3);
    assert_eq!(page_texts(h.state()), ["Page 1", "Page 3"]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert_eq!(page_texts(h.state()), ["Page 1", "Page 2", "Page 3"]);
    let single = organize(1);
    assert!(single.get_by_label("Delete page 1").accesskit_node().is_disabled());
}

#[test]
fn organize_local_actions_respect_document_permissions() {
    let mut h = harness(3, |app| {
        app.apply_edit(pdfcraft_engine::Edit::Protect(pdfcraft_engine::Protection {
            permissions_password: Some("owner".into()),
            changes: pdfcraft_engine::Changes::None,
            ..Default::default()
        }));
        let bytes = app.session.save_bytes(app.views[0].id).unwrap();
        app.open_bytes("restricted.pdf", None, bytes.as_ref().clone()).unwrap();
        app.set_option("tool", "organize").unwrap();
    });
    h.get_by_label("Select page 2").click();
    h.run_steps(2);
    assert!(h.get_by_label("Rotate page 2 clockwise").accesskit_node().is_disabled());
    assert!(h.get_by_label("Rotate page 2 counterclockwise").accesskit_node().is_disabled());
    assert!(h.get_by_label("Delete page 2").accesskit_node().is_disabled());
    let app = h.state();
    let view = &app.views[app.active.unwrap()];
    assert_eq!(view.target_pages(), [1]);
    assert!(!app.session.get(view.id).unwrap().dirty);
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
    let mut empty = pdfcraft_ui_egui::canvas::DocView::new(pdfcraft_engine::DocId(0), &Default::default(), Default::default());
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
    insert_menu(&mut h, "Insert a blank page after the selection");
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
fn keys_leave_the_document_under_a_dialog_alone() {
    // Issue #870: ⌘W with Preferences open closed the file underneath it, and in the page grid
    // Delete deleted the selected page. While a dialog is open its keys are its own.
    let mut h = organize(3);
    h.get_by_label("Page 2").click();
    h.run_steps(2);
    h.get_by_label("Rotate clockwise").click();
    h.run_steps(3);
    assert!(h.state_mut().execute("app.preferences"));
    h.run_steps(3);
    let shift = Modifiers::COMMAND | Modifiers::SHIFT;
    for (m, key) in [
        (Modifiers::COMMAND, Key::W),
        (shift, Key::W),
        (Modifiers::COMMAND, Key::Z),
        (Modifiers::COMMAND, Key::D),
        (Modifiers::COMMAND, Key::F),
        (Modifiers::NONE, Key::Delete),
        (Modifiers::NONE, Key::Backspace),
    ] {
        h.key_press_modifiers(m, key);
        h.run_steps(3);
        let app = h.state();
        assert_eq!(app.views.len(), 1, "{m:?}+{key:?} closed the file");
        assert!(app.close_request.is_none(), "{m:?}+{key:?} asked to close the file");
        assert!(app.session.get(app.views[0].id).unwrap().can_undo().is_some(), "{m:?}+{key:?} undid the rotation");
        assert!(app.views[0].find.is_none(), "{m:?}+{key:?} opened Find");
        assert_eq!(app.dialog, Some(Dialog::Preferences), "{m:?}+{key:?} replaced Preferences");
    }
    assert_eq!(page_texts(h.state()), ["Page 1", "Page 2", "Page 3"], "a page was deleted under the dialog");
    h.key_press(Key::Escape);
    h.run_steps(3);
    assert!(h.state().dialog.is_none(), "Escape closes Preferences");
    assert_eq!(h.state().views[0].target_pages(), [1], "Escape went to the dialog, not the page selection");
    h.key_press_modifiers(Modifiers::COMMAND, Key::W);
    h.run_steps(3);
    assert!(h.state().close_request.is_some(), "with the dialog closed, ⌘W closes the file again");
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

/// PDFs written to a fresh temp folder, removed when the test ends.
struct TempPdfs(std::path::PathBuf);

impl TempPdfs {
    fn new(test: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("pdfcraft-session-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn file(&self, name: &str, pages: usize) -> String {
        let path = self.0.join(name);
        std::fs::write(&path, fixture(pages)).unwrap();
        path.to_string_lossy().into_owned()
    }
}

impl Drop for TempPdfs {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The app with Preferences ▸ Reopen the files that were open when PdfCraft last closed on.
fn session_harness(setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.reopen_last_session = true;
        setup(&mut app);
        app
    });
    h.run_steps(4);
    h
}

fn open_paths(app: &PdfCraftApp) -> Vec<String> {
    app.views.iter().map(|v| app.session.get(v.id).and_then(|d| d.path.clone()).unwrap_or_default()).collect()
}

#[test]
fn the_last_session_reopens_its_files_pages_and_active_tab() {
    let dir = TempPdfs::new("reopen");
    let files = [dir.file("a.pdf", 1), dir.file("b.pdf", 5), dir.file("c.pdf", 2)];
    let opened = files.clone();
    let mut h = session_harness(move |app| opened.iter().for_each(|f| app.open_path(f)));
    h.state_mut().views[1].go_to_page(3);
    h.state_mut().active = Some(1);
    h.run_steps(4);
    assert_eq!(h.state().views[1].current, 3, "b.pdf shows page 4");
    let settings = h.state().persist();
    let mut h = session_harness(move |app| {
        app.restore(&settings);
        app.reopen_last_files(&[]);
    });
    h.run_steps(4);
    let app = h.state();
    assert_eq!(open_paths(app), files, "the same files, in the same order");
    assert_eq!(app.active, Some(1), "b.pdf is in front again");
    assert_eq!(app.views[1].current, 3, "at the page it showed");
}

#[test]
fn quitting_with_unsaved_changes_still_remembers_every_file() {
    let dir = TempPdfs::new("quit");
    let files = [dir.file("a.pdf", 1), dir.file("b.pdf", 1)];
    let opened = files.clone();
    let mut h = session_harness(move |app| opened.iter().for_each(|f| app.open_path(f)));
    for tab in 0..2 {
        h.state_mut().active = Some(tab);
        h.state_mut().views[tab].select_pages(&[0]);
        h.state_mut().apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
    }
    h.state_mut().active = Some(1);
    h.state_mut().close_request = Some(CloseRequest::Quit);
    h.run_steps(3);
    // Each unsaved tab closes as it is answered, before PdfCraft quits.
    for _ in 0..2 {
        h.get_by_label("Don't save").click();
        h.run_steps(3);
    }
    assert!(h.state().views.is_empty());
    let mut app = PdfCraftApp::new();
    app.restore(&h.state().persist());
    app.reopen_last_files(&[]);
    assert_eq!(open_paths(&app), files, "both files come back");
    assert_eq!(app.active, Some(1), "with b.pdf in front, as when the quit began");
}

#[test]
fn the_last_session_is_off_by_default_and_skips_missing_and_duplicate_files() {
    let dir = TempPdfs::new("skip");
    let (a, b) = (dir.file("a.pdf", 1), dir.file("b.pdf", 1));
    let mut app = PdfCraftApp::new();
    app.open_path(&a);
    app.open_path(&b);
    let off: serde_json::Value = serde_json::from_str(&app.persist()).unwrap();
    assert!(off["last_session"].is_null(), "nothing is kept while the preference is off");
    app.reopen_last_session = true;
    let settings = app.persist();
    let mut later = PdfCraftApp::new();
    later.restore(&settings);
    later.reopen_last_session = false;
    later.reopen_last_files(&[]);
    assert!(later.views.is_empty(), "turned off again: nothing reopens");
    // b.pdf is gone, and a.pdf is about to open from the command line.
    std::fs::remove_file(&b).unwrap();
    let mut later = PdfCraftApp::new();
    later.restore(&settings);
    later.reopen_last_files(&[a]);
    assert!(later.views.is_empty(), "neither opens from the session");
}

#[test]
fn save_prompt_stays_inside_the_screen_for_a_long_filename() {
    // Issue #161: an unwrapped title carrying a long filename widened the centered modal past
    // the viewport, clipping the message and pushing the Save/Cancel buttons off-screen.
    let name = "Psychology_ The Science of Mind and Behaviour, -- Nigel Holt, Andy Bremner, Michael \
                Vliek, Ed Sutherland, -- 5, 2024 -- McGraw-Hill Education (UK) Ltd -- isbn13 97815268.pdf";
    let mut h = Harness::builder().with_size(egui::vec2(1365.0, 719.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes(name, None, fixture(1)).expect("fixture opens");
        app.close_request = Some(CloseRequest::Tab(app.views[0].id));
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
fn save_prompt_fits_the_smallest_window_whatever_the_name() {
    // Issue #236: wrapping (#161) kept the prompt narrow, but a long enough name still grew it
    // taller than the window and pushed the buttons off-screen. On the web a `?file=` URL names
    // the document, so the name has no length limit. Long names now give way in the middle.
    let names = [
        format!("{}.pdf", "a".repeat(400)),
        format!("{}.pdf", "Quarterly_Report_FY2026_Final_v3_".repeat(12)),
        format!("{}.pdf", "รายงานประจำปีงบประมาณ".repeat(15)),
        format!("{}.pdf", "年".repeat(251)),
        format!("{}.pdf", "W".repeat(2000)),
    ];
    // The desktop window's minimum inner size (apps/pdfcraft/src/main.rs).
    let size = egui::vec2(820.0, 520.0);
    for name in names {
        let start: String = name.chars().take(10).collect();
        let mut h = Harness::builder().with_size(size).build_eframe(move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.open_bytes(&name, None, fixture(1)).expect("fixture opens");
            app.close_request = Some(CloseRequest::Tab(app.views[0].id));
            app
        });
        h.run_steps(4);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        let inside = |r: egui::Rect| screen.contains(r.min) && screen.contains(r.max);
        let title = h.get_by_label_contains("Save changes to");
        let title_rect = title.rect();
        assert!(inside(title_rect), "{start}…: the title rect {title_rect:?} leaves the screen");
        let opening = format!("Save changes to “{start}");
        assert!(
            h.query_by(|n| n.value().is_some_and(|l| l.starts_with(&opening) && l.contains('…') && l.contains(".pdf” before closing?"))).is_some(),
            "{start}…: the title keeps the start and the extension"
        );
        for button in ["Save", "Cancel", "Don't save"] {
            let rect = h.get_by_label(button).rect();
            assert!(inside(rect), "{start}…: the {button} button rect {rect:?} leaves the screen");
        }
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

/// A one-page PDF whose form holds one text field named `name`.
fn form_pdf(name: &str) -> Vec<u8> {
    form_pdf_pages(name, 1)
}

/// A PDF of `pages` pages whose form holds one text field named `name` (on the first page).
fn form_pdf_pages(name: &str, pages: usize) -> Vec<u8> {
    let kids: Vec<String> = (0..pages).map(|i| format!("{} 0 R", 3 + i)).collect();
    let mut objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] >> >>".into(),
        format!("<< /Type /Pages /Kids [{}] /Count {pages} /MediaBox [0 0 300 400] >>", kids.join(" ")),
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>".into(),
        format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T ({name}) /V (x) /Rect [60 60 200 80] /P 3 0 R >>"),
    ];
    for _ in 1..pages {
        objs.push("<< /Type /Page /Parent 2 0 R >>".to_string()); // objects after the field's number
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

/// organize.field-name-conflicts: the Combine tab warns, before anything is combined, that a
/// file's form fields collide with earlier files' and will be renamed, naming the new names.
#[test]
fn combine_list_warns_that_colliding_field_names_will_be_renamed() {
    let mut h = harness(1, |app| {
        app.set_option("combine-view", "list").unwrap();
        app.use_files(pdfcraft_ui_egui::FilePurpose::Combine, vec![("one.pdf".into(), form_pdf("Text1")), ("two.pdf".into(), form_pdf("Text1"))]);
    });
    h.run_steps(3);
    h.get_by_label_contains("will be renamed");
    h.get_by_label_contains("Text1 → Text1-1");
}

/// A file split into parts (its pages moved among other files') is combined as one file, once —
/// so the preview must not warn that its field names collide with their own.
#[test]
fn combine_list_does_not_warn_that_a_split_file_collides_with_itself() {
    let mut h = harness(1, |app| {
        app.set_option("combine-view", "list").unwrap();
        app.use_files(pdfcraft_ui_egui::FilePurpose::Combine, vec![("one.pdf".into(), form_pdf_pages("Text1", 2))]);
    });
    h.run_steps(3);
    // one.pdf in two parts — pages 1 and 2, sharing a group, as moving its pages apart leaves it.
    let mut part = h.state().combine_draft[0].clone();
    part.id += 1000;
    h.state_mut().combine_draft[0].range = "1".into();
    part.range = "2".into();
    h.state_mut().combine_draft.push(part);
    h.run_steps(3);
    assert!(h.query_by_label_contains("will be renamed").is_none(), "one file, however many parts, cannot collide with itself");
}

/// Copying a page with a field and pasting it back into the same document is a duplicate: the
/// copy keeps the field's name rather than becoming Text1-1.
#[test]
fn pasting_pages_into_their_own_document_keeps_field_names() {
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    app.open_bytes("form.pdf", None, form_pdf("Text1")).unwrap();
    app.views[0].select_pages(&[0]);
    assert!(app.execute("page.copy"));
    assert!(app.execute("page.paste"));
    let doc = app.session.get(app.views[0].id).unwrap();
    assert_eq!(doc.info.pages.len(), 2, "pasted");
    assert!(doc.form.iter().all(|f| f.name == "Text1"), "{:?}", doc.form.iter().map(|f| &f.name).collect::<Vec<_>>());
}

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
        app.set_option("combine-view", "list").unwrap();
        app.use_files(pdfcraft_ui_egui::FilePurpose::Combine, vec![("one.pdf".into(), fixture(3)), ("two.pdf".into(), fixture(2))]);
    });
    h.run_steps(3);
    h.get_by_label_contains("Files are combined from top to bottom");
    // Nothing is selected yet: the toolbar's Move up is there but disabled.
    assert!(h.get_by_label("Move up").accesskit_node().is_disabled());
    // Select two.pdf with the keyboard and move it up: two.pdf first; one.pdf's pages 3 and 1.
    h.key_press(Key::ArrowUp);
    h.run_steps(2);
    assert_eq!(h.state().combine_selection(), [1]);
    h.get_by_label("Move up").click();
    h.run_steps(2);
    assert_eq!(h.state().combine_draft[0].name, "two.pdf");
    assert_eq!(h.state().combine_selection(), [0], "the selection follows the moved file");
    h.state_mut().combine_draft[1].range = "3, 1".into();
    h.run_steps(2);
    h.get_by_label("2 of 3");
    h.get_by_label("Combine").click();
    h.run_steps(3);
    let app = h.state();
    assert_eq!(texts_of(app, 1), ["Page 1", "Page 2", "Page 3", "Page 1"]);
    let doc = app.session.get(app.views[1].id).unwrap();
    assert_eq!(doc.info.outline.iter().map(|o| o.title.as_str()).collect::<Vec<_>>(), ["two", "one"]);
    assert!(app.combine_draft.is_empty());
    assert!(!app.combine_tab.open, "the tab makes way for the result");
}

#[test]
fn combine_files_opens_in_its_own_tab() {
    let mut h = harness(1, |_| {});
    h.state_mut().execute("page.combine");
    h.run_steps(3);
    assert!(h.state().combine_showing() && h.state().active.is_none());
    h.get_by_label("Add the files to combine");
    // Back to the document, then to Combine through its tab.
    h.get_by_label("doc.pdf").click();
    h.run_steps(3);
    assert_eq!(h.state().active, Some(0));
    assert!(!h.state().combine_showing() && h.state().combine_tab.open);
    // The tab, in the tab strip (All tools lists Combine files too).
    h.get_all_by_label("Combine files").find(|n| n.rect().top() < 40.0).unwrap().click();
    h.run_steps(3);
    assert!(h.state().combine_showing());
    // Home hides it too.
    h.get_by_label("Home").click();
    h.run_steps(3);
    assert!(!h.state().combine_showing() && h.state().combine_tab.open);
}

#[test]
fn closing_the_combine_tab_forgets_its_list() {
    let mut h = harness(1, |app| {
        app.use_files(pdfcraft_ui_egui::FilePurpose::Combine, vec![("one.pdf".into(), fixture(1))]);
    });
    h.run_steps(3);
    assert!(h.state().combine_showing());
    h.state_mut().close_combine_tab();
    h.run_steps(3);
    let app = h.state();
    assert!(app.combine_draft.is_empty() && !app.combine_tab.open);
    assert_eq!(app.active, Some(0), "the document next to it shows");
}

#[test]
fn combine_lists_size_and_warns_before_combining() {
    let mut h = harness(1, |app| {
        app.set_option("combine-view", "list").unwrap();
        app.use_files(
            pdfcraft_ui_egui::FilePurpose::Combine,
            vec![
                ("one.pdf".into(), fixture(3)),
                ("locked.pdf".into(), protected("", "owner", -1 ^ 1024 ^ 8)),
                ("secret.pdf".into(), protected("pw", "owner", -1)),
            ],
        );
    });
    h.run_steps(3);
    h.get_by_label("File name");
    h.get_by_label("Warnings");
    h.get_by_label("Its security settings don't allow copying pages");
    h.get_by_label("Password-protected");
    h.get_by_label("2 files need attention");
    assert!(h.get_by_label("Combine").accesskit_node().is_disabled());
    // Removing them (Delete on the selection) lets Combine run.
    for _ in 0..2 {
        h.state_mut().select_combine_rows(&[1]);
        h.key_press(Key::Delete);
        h.run_steps(2);
    }
    assert_eq!(h.state().combine_draft.len(), 1);
    h.get_by_label_contains("1 file · 3 pages");
}

#[test]
fn a_bad_range_stops_combine_until_fixed() {
    let mut h = harness(1, |app| {
        app.use_files(pdfcraft_ui_egui::FilePurpose::Combine, vec![("one.pdf".into(), fixture(3)), ("two.pdf".into(), fixture(2))]);
    });
    h.run_steps(3);
    h.get_by_label_contains("2 files · 5 pages");
    h.state_mut().combine_draft[0].range = "1-99".into();
    h.run_steps(2);
    h.get_by_label("1 file needs attention");
    assert!(h.get_by_label("Combine").accesskit_node().is_disabled());
    h.state_mut().combine_draft[0].range = "2-3".into();
    h.run_steps(2);
    h.get_by_label_contains("2 files · 4 pages");
    assert!(!h.get_by_label("Combine").accesskit_node().is_disabled());
}

#[test]
fn alt_arrows_move_the_selected_file() {
    let mut h = harness(1, |app| {
        app.use_files(
            pdfcraft_ui_egui::FilePurpose::Combine,
            vec![("a.pdf".into(), fixture(1)), ("b.pdf".into(), fixture(1)), ("c.pdf".into(), fixture(1))],
        );
    });
    h.run_steps(3);
    h.key_press(Key::ArrowDown);
    h.run_steps(1);
    h.key_press_modifiers(Modifiers::ALT, Key::ArrowDown);
    h.run_steps(2);
    let names: Vec<_> = h.state().combine_draft.iter().map(|f| f.name.clone()).collect();
    assert_eq!(names, ["b.pdf", "a.pdf", "c.pdf"]);
    assert_eq!(h.state().combine_selection(), [1]);
}

fn combine_names(h: &Harness<'static, PdfCraftApp>) -> Vec<String> {
    h.state().combine_draft.iter().map(|f| f.name.clone()).collect()
}

/// Files in the Combine tab's list view (the table: headings, page ranges, row links).
fn combine_of(names: &[(&str, usize)]) -> Harness<'static, PdfCraftApp> {
    let files: Vec<(String, Vec<u8>)> = names.iter().map(|(n, p)| (n.to_string(), fixture(*p))).collect();
    let mut h = harness(1, move |app| {
        app.set_option("combine-view", "list").unwrap();
        app.use_files(pdfcraft_ui_egui::FilePurpose::Combine, files);
    });
    h.run_steps(3);
    h
}

#[test]
fn column_headings_sort_the_list_and_flip_on_the_second_click() {
    let mut h = combine_of(&[("scan10.pdf", 1), ("Scan2.pdf", 3), ("a.pdf", 2)]);
    h.get_by_label("File name").click();
    h.run_steps(2);
    assert_eq!(combine_names(&h), ["a.pdf", "Scan2.pdf", "scan10.pdf"], "case-insensitive, numbers by value");
    h.get_by_label("File name, sorted ascending").click();
    h.run_steps(2);
    assert_eq!(combine_names(&h), ["scan10.pdf", "Scan2.pdf", "a.pdf"]);
    h.get_by_label("Pages").click();
    h.run_steps(2);
    assert_eq!(combine_names(&h), ["scan10.pdf", "a.pdf", "Scan2.pdf"]);
    // Moving a file by hand ends the sorted order.
    h.state_mut().select_combine_rows(&[2]);
    h.get_by_label("Move up").click();
    h.run_steps(2);
    assert_eq!(h.state().combine_tab.sort, None);
    h.get_by_label("Pages");
}

#[test]
fn ctrl_and_shift_clicks_select_several_files_that_move_and_go_together() {
    let mut h = combine_of(&[("a.pdf", 1), ("b.pdf", 1), ("c.pdf", 1), ("d.pdf", 1), ("e.pdf", 1)]);
    h.get_by_label("b.pdf").click();
    h.run_steps(1);
    h.get_by_label("d.pdf").click_modifiers(Modifiers::SHIFT);
    h.run_steps(1);
    assert_eq!(h.state().combine_selection(), [1, 2, 3]);
    h.get_by_label("c.pdf").click_modifiers(Modifiers::COMMAND);
    h.run_steps(1);
    assert_eq!(h.state().combine_selection(), [1, 3], "Ctrl/⌘ toggles one file");
    h.get_by_label("2 selected");
    h.get_by_label("Move up").click();
    h.run_steps(2);
    assert_eq!(combine_names(&h), ["b.pdf", "a.pdf", "d.pdf", "c.pdf", "e.pdf"]);
    h.get_by_label("Remove 2 files").click();
    h.run_steps(2);
    assert_eq!(combine_names(&h), ["a.pdf", "c.pdf", "e.pdf"]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(1);
    assert_eq!(h.state().combine_selection(), [0, 1, 2]);
}

#[test]
fn undo_and_redo_restore_the_combine_list() {
    let mut h = combine_of(&[("a.pdf", 1), ("b.pdf", 1), ("c.pdf", 1)]);
    h.state_mut().select_combine_rows(&[0, 2]);
    h.key_press(Key::Delete);
    h.run_steps(2);
    assert_eq!(combine_names(&h), ["b.pdf"]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(2);
    assert_eq!(combine_names(&h), ["a.pdf", "b.pdf", "c.pdf"], "⌘Z brings the removed files back");
    assert_eq!(h.state().combine_selection(), [0, 2], "…selected as they were");
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    h.run_steps(2);
    assert_eq!(combine_names(&h), ["b.pdf"]);
    // The toolbar's buttons do the same; undo also reverses a sort and the files' addition.
    h.get_by_label("Undo").click();
    h.run_steps(2);
    h.get_by_label("File name").click();
    h.run_steps(2);
    h.get_by_label("File name, sorted ascending").click();
    h.run_steps(2);
    assert_eq!(combine_names(&h), ["c.pdf", "b.pdf", "a.pdf"]);
    h.get_by_label("Undo").click();
    h.run_steps(2);
    assert_eq!(combine_names(&h), ["a.pdf", "b.pdf", "c.pdf"]);
    // A page range typed in is one step.
    h.state_mut().combine_draft[0].range.clear();
    let field = h.get_all_by_role(Role::TextInput).next().unwrap();
    field.focus();
    h.run_steps(1);
    h.get_all_by_role(Role::TextInput).next().unwrap().type_text("1");
    h.run_steps(2);
    assert_eq!(h.state().combine_draft[0].range, "1");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(2);
    assert_eq!(h.state().combine_draft[0].range, "");
    // The document's own history is untouched: it has nothing to undo.
    h.get_by_label("doc.pdf").click();
    h.run_steps(2);
    assert!(h.state().session.get(h.state().views[0].id).unwrap().can_undo().is_none());
}

#[test]
fn protected_files_that_can_be_combined_are_flagged() {
    let mut h = harness(1, |app| {
        app.set_option("combine-view", "list").unwrap();
        app.use_files(
            pdfcraft_ui_egui::FilePurpose::Combine,
            vec![("one.pdf".into(), fixture(1)), ("no-copy.pdf".into(), protected("", "owner", -1 ^ 16))],
        );
    });
    h.run_steps(3);
    let app = h.state();
    assert!(app.combine_draft[1].problem.is_none(), "copying text is withheld, assembling pages isn't");
    h.get_by_label("Protected: the combined file won't keep its security settings");
    assert!(!h.get_by_label("Combine").accesskit_node().is_disabled(), "a warning, not a blocker");
}

#[test]
fn the_whole_heading_sorts_and_dragging_it_moves_the_column() {
    let mut h = combine_of(&[("b.pdf", 1), ("a.pdf", 3)]);
    // A click in the empty right part of the heading cell, away from its text.
    let cell = h.get_by_label("File name").rect();
    let spot = egui::pos2(cell.right() - 30.0, cell.center().y);
    h.hover_at(spot);
    h.run_steps(1);
    h.drag_at(spot);
    h.run_steps(1);
    h.drop_at(spot);
    h.run_steps(2);
    assert_eq!(combine_names(&h), ["a.pdf", "b.pdf"]);
    // Drag Size before File name.
    let size = h.get_by_label("Size").rect().center();
    let name = h.get_by_label("File name, sorted ascending").rect();
    drag(&mut h, size, egui::pos2(name.left() + 10.0, name.center().y));
    use pdfcraft_ui_egui::SortKey::*;
    assert_eq!(h.state().combine_columns.order, [Size, Name, Pages, Modified, Warnings]);
    assert_eq!(combine_names(&h), ["a.pdf", "b.pdf"], "moving a column doesn't sort");
    let (size, name) = (h.get_by_label("Size").rect(), h.get_by_label("File name, sorted ascending").rect());
    assert!(size.left() < name.left(), "Size now shows first");
}

#[test]
fn columns_resize_and_the_layout_is_kept_in_the_settings() {
    let mut h = combine_of(&[("a.pdf", 1), ("b.pdf", 1)]);
    let size = h.get_by_label("Size").rect();
    // The separator after Size: drag it right by 60 points.
    let edge = egui::pos2(size.right() + 2.0, size.center().y);
    drag(&mut h, edge, edge + egui::vec2(60.0, 0.0));
    let widths = h.state().combine_columns.widths.expect("a resized column keeps its width");
    let size_index = pdfcraft_ui_egui::SortKey::Size as usize;
    assert!(widths[size_index] > 120.0, "{widths:?}");
    let saved = h.state().persist();
    let mut fresh = PdfCraftApp::new();
    fresh.set_option("language", "en").unwrap();
    fresh.restore(&saved);
    assert_eq!(fresh.combine_columns, h.state().combine_columns);
    // Malformed settings give the default layout.
    fresh.restore(r#"{"combine_columns":{"order":["size","size"],"widths":[1,2]}}"#);
    assert_eq!(fresh.combine_columns, Default::default());
}

#[test]
fn a_folder_adds_its_pdfs_images_and_text_in_order_and_optionally_its_subfolders() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-combine-folder-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    for (name, pages) in [("page10.pdf", 1), ("page2.PDF", 2), ("sub/inner.pdf", 3)] {
        std::fs::write(dir.join(name), fixture(pages)).unwrap();
    }
    // #569: images and text files join too; other files are left out.
    std::fs::write(dir.join("notes.txt"), b"Some notes").unwrap();
    std::fs::write(dir.join("readme.md"), b"# not combinable").unwrap();
    let mut h = harness(1, |_| {});
    h.state_mut().execute("page.combine");
    h.run_steps(2);
    h.state_mut().pick_override = Some(vec![dir.to_string_lossy().into_owned()]);
    h.get_by_label("Add folder…").click();
    h.run_steps(4);
    assert_eq!(combine_names(&h), ["notes.txt", "page2.PDF", "page10.pdf"], "numbers by value");
    // The toolbar's menu: the folder and its subfolders, as one undo step.
    h.get_by_label("More ways to add files").click();
    h.run_steps(2);
    h.get_by_label("Add folder and subfolders…").click();
    h.run_steps(4);
    assert_eq!(combine_names(&h), ["notes.txt", "page2.PDF", "page10.pdf", "notes.txt", "page2.PDF", "page10.pdf", "inner.pdf"]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(2);
    assert_eq!(combine_names(&h).len(), 3);
    let _ = std::fs::remove_dir_all(&dir);
}

/// #569: an image or a text file added to Combine files becomes a PDF as it joins the list, an
/// ordinary row with its pages.
#[test]
fn images_and_text_files_join_the_combine_list_as_pdfs() {
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbImage::from_pixel(40, 20, image::Rgb([90, 120, 200])).write_to(&mut png, image::ImageFormat::Png).unwrap();
    let mut h = combine_of(&[("one.pdf", 2)]);
    h.state_mut()
        .use_files(pdfcraft_ui_egui::FilePurpose::Combine, vec![("scan.png".into(), png.into_inner()), ("notes.txt".into(), b"Some notes".to_vec())]);
    h.run_steps(3);
    assert_eq!(combine_names(&h), ["one.pdf", "scan.png", "notes.txt"]);
    let draft = &h.state().combine_draft;
    assert!(draft.iter().all(|f| f.bytes.starts_with(b"%PDF-") && f.problem.is_none()), "made into PDFs");
    assert_eq!(draft.iter().map(|f| f.pages).collect::<Vec<_>>(), [2, 1, 1]);
    h.get_by_label("Combine").click();
    h.run_steps(3);
    assert_eq!(texts_of(h.state(), 1), ["Page 1", "Page 2", "", "Some notes"]);
}

/// A file that can't be read (a damaged image, a type that can't be made into a PDF) joins the
/// list all the same, with why; Combine asks before leaving it out, and Cancel changes nothing.
#[test]
fn files_that_cannot_be_read_are_listed_and_left_out_once_the_user_agrees() {
    let mut h = combine_of(&[("one.pdf", 2)]);
    let bad = vec![("broken.jpg".to_string(), b"\xff\xd8\xff\xe0 not a JPEG".to_vec()), ("report.docx".to_string(), b"PK\x03\x04".to_vec())];
    h.state_mut().use_files(pdfcraft_ui_egui::FilePurpose::Combine, bad);
    h.run_steps(3);
    assert_eq!(combine_names(&h), ["one.pdf", "broken.jpg", "report.docx"]);
    for f in &h.state().combine_draft[1..] {
        let why = f.problem.as_deref().expect("why it can't be combined");
        assert!(why.starts_with("Can't be read: ") && !why.contains(&f.name), "{why}");
        assert!(f.lock.is_none() && f.pages == 0);
    }
    h.get_by_label_contains("this file type can't be converted");
    // One readable file is not enough to combine.
    assert!(h.get_by_label("Combine").accesskit_node().is_disabled());
    h.state_mut().use_files(pdfcraft_ui_egui::FilePurpose::Combine, vec![("two.pdf".into(), fixture(1))]);
    h.run_steps(3);
    assert!(!h.get_by_label("Combine").accesskit_node().is_disabled());
    // Combine asks first, naming the files left out; Cancel leaves everything as it was.
    h.get_by_label("Combine").click();
    h.run_steps(3);
    h.get_by_label("Leave out files that can't be read?");
    h.get_by_label("• broken.jpg");
    h.get_by_label("• report.docx");
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert!(h.query_by_label("Leave out files that can't be read?").is_none());
    assert_eq!(h.state().views.len(), 1, "nothing combined");
    assert_eq!(combine_names(&h).len(), 4);
    // Asked again: Combine the rest.
    h.get_by_label("Combine").click();
    h.run_steps(3);
    h.get_by_label("Combine the rest").click();
    h.run_steps(3);
    assert_eq!(texts_of(h.state(), 1), ["Page 1", "Page 2", "Page 1"]);
}

fn type_password(h: &mut Harness<'static, PdfCraftApp>, password: &str) {
    let field = h.get_by_role(Role::PasswordInput);
    field.focus();
    field.type_text(password);
    h.run_steps(1);
    h.key_press(Key::Enter);
    h.run_steps(3);
}

#[test]
fn a_password_protected_file_is_unlocked_with_its_password_and_combined() {
    let mut h = combine_of(&[("one.pdf", 1)]);
    h.state_mut().use_files(pdfcraft_ui_egui::FilePurpose::Combine, vec![("secret.pdf".into(), protected("pw", "owner", -1))]);
    h.run_steps(3);
    h.get_by_label("Password-protected");
    assert!(h.get_by_label("Combine").accesskit_node().is_disabled());
    // The row's link (the toolbar's Unlock… comes first).
    h.get_all_by_label("Unlock…").last().unwrap().click();
    h.run_steps(3);
    h.get_by_label("Unlock secret.pdf");
    type_password(&mut h, "nope");
    h.get_by_label("Wrong password");
    assert!(h.state().combine_draft[1].lock.is_some());
    // The box stays open for another try.
    let field = h.get_by_role(Role::PasswordInput);
    field.focus();
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(1);
    type_password(&mut h, "pw");
    let f = &h.state().combine_draft[1];
    assert!(f.lock.is_none() && f.problem.is_none());
    assert_eq!(f.pages, 2);
    h.get_by_label("Unlocked: the combined file won't be password-protected");
    // Never printed: debug output ends up in logs.
    assert!(!format!("{:?} {:?}", h.state().combine_draft, h.state().combine_tab).contains("pw\""));
    h.get_by_label("Combine").click();
    h.run_steps(3);
    assert_eq!(texts_of(h.state(), 1), ["Page 1", "Page 1", "Page 2"]);
}

#[test]
fn one_password_unlocks_several_selected_files_and_undo_locks_them_again() {
    let mut h = combine_of(&[("one.pdf", 1)]);
    h.state_mut().use_files(
        pdfcraft_ui_egui::FilePurpose::Combine,
        vec![
            ("a.pdf".into(), protected("same", "o1", -1)),
            ("b.pdf".into(), protected("same", "o2", -1)),
            ("c.pdf".into(), protected("other", "o3", -1)),
        ],
    );
    h.run_steps(3);
    h.state_mut().select_combine_rows(&[1, 2, 3]);
    h.run_steps(1);
    h.get_all_by_label("Unlock…").next().unwrap().click(); // the toolbar's (the rows' links come after it)
    h.run_steps(3);
    h.get_by_label("Unlock 3 files");
    type_password(&mut h, "same");
    let locked: Vec<bool> = h.state().combine_draft.iter().map(|f| f.lock.is_some()).collect();
    assert_eq!(locked, [false, false, false, true]);
    h.get_by_label_contains("Unlocked 2 of 3 files");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(2);
    assert!(h.state().combine_draft[1..].iter().all(|f| f.lock.is_some()), "undo locks them again");
}

#[test]
fn a_file_that_opens_but_forbids_combining_takes_the_permissions_password() {
    let mut h = combine_of(&[("one.pdf", 1)]);
    // Opens with "pw"; only the owner may assemble pages.
    h.state_mut().use_files(pdfcraft_ui_egui::FilePurpose::Combine, vec![("both.pdf".into(), protected("pw", "owner", -1 ^ 1024 ^ 8))]);
    h.run_steps(3);
    assert!(h.state_mut().combine_unlock_rows(&[1], "pw"));
    h.run_steps(2);
    assert_eq!(h.state().combine_draft[1].lock, Some(pdfcraft_ui_egui::CombineLock::Permissions));
    h.get_by_label_contains("Enter the permissions password");
    h.get_all_by_label("Unlock…").last().unwrap().click();
    h.run_steps(3);
    h.get_by_label("Enter the permissions password to allow combining");
    type_password(&mut h, "owner");
    assert!(h.state().combine_draft[1].lock.is_none());
    assert!(!h.get_by_label("Combine").accesskit_node().is_disabled());
}

#[test]
fn an_rc4_file_unlocks_with_its_owner_password_too() {
    // The engine reads RC4 files with their owner password; the inspector may not, and the
    // engine's answer is the one that counts.
    let mut h = combine_of(&[("one.pdf", 1)]);
    h.state_mut().use_files(
        pdfcraft_ui_egui::FilePurpose::Combine,
        vec![("rc4.pdf".into(), protected_with(pdfcraft_cos::Algorithm::Rc4_128, "user", "boss", -1))],
    );
    h.run_steps(3);
    assert!(h.state_mut().combine_unlock_rows(&[1], "boss"));
    let f = &h.state().combine_draft[1];
    assert!(f.lock.is_none());
    assert_eq!(f.pages, 2);
}

#[test]
fn open_documents_can_be_added_to_combine() {
    let mut h = harness(2, |_| {});
    h.state_mut().execute("page.combine");
    h.run_steps(3);
    h.get_by_label("Add open documents").click();
    h.run_steps(2);
    // The menu's entry, after the tab of the same name.
    h.get_all_by_label("doc.pdf").last().unwrap().click();
    h.run_steps(3);
    let app = h.state();
    assert_eq!(app.combine_draft.len(), 1);
    assert_eq!(app.combine_draft[0].pages, 2);
    assert!(app.combine_showing());
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
fn replace_button_replaces_selected_pages_in_organize() {
    let dir = temp_path("replace-btn");
    std::fs::create_dir_all(&dir).unwrap();
    let other = dir.join("other.pdf");
    std::fs::write(&other, fixture(5)).unwrap();

    let mut h = organize(3);
    h.get_by_label("Page 2").click();
    h.run_steps(1);
    h.get_by_label("Page 3").click_modifiers(Modifiers::COMMAND);
    h.run_steps(2);
    h.state_mut().pick_override = Some(vec![other.to_string_lossy().into_owned()]);
    h.get_by_label("Replace").click();
    h.run_steps(3);
    egui_kittest::kittest::Queryable::get_by_label(&h, "Replace Pages");
    assert_eq!(h.state().replace_draft.as_ref().unwrap().from, 2);
    assert_eq!(h.state().replace_draft.as_ref().unwrap().to, 3);
    egui_kittest::kittest::Queryable::get_by_label(&h, "OK").click();
    h.run_steps(3);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(doc.info.pages.len(), 3);
    assert_eq!(doc.can_undo(), Some("Replace pages"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn replace_pages_from_thumbnail_context_menu() {
    let dir = temp_path("replace-ctx");
    std::fs::create_dir_all(&dir).unwrap();
    let other = dir.join("other.pdf");
    std::fs::write(&other, fixture(5)).unwrap();

    let mut h = organize(3);
    h.state_mut().pick_override = Some(vec![other.to_string_lossy().into_owned()]);
    h.get_by_label("Page 2").click_button(egui::PointerButton::Secondary);
    h.run_steps(2);
    h.get_all_by_label("Replace pages…").last().unwrap().click();
    h.run_steps(3);
    egui_kittest::kittest::Queryable::get_by_label(&h, "Replace Pages");
    assert_eq!(h.state().replace_draft.as_ref().unwrap().from, 2);
    assert_eq!(h.state().replace_draft.as_ref().unwrap().to, 2);
    egui_kittest::kittest::Queryable::get_by_label(&h, "OK").click();
    h.run_steps(3);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(doc.info.pages.len(), 3);
    assert_eq!(doc.can_undo(), Some("Replace page"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn replace_pages_from_pages_panel_context_menu() {
    let dir = temp_path("replace-panel-ctx");
    std::fs::create_dir_all(&dir).unwrap();
    let other = dir.join("other.pdf");
    std::fs::write(&other, fixture(5)).unwrap();

    let mut h = harness(4, |app| app.right = Some(pdfcraft_ui_egui::RightPanel::Pages));
    h.state_mut().pick_override = Some(vec![other.to_string_lossy().into_owned()]);
    let thumb = h.get_all_by_label("Page 2").max_by(|a, b| a.rect().left().total_cmp(&b.rect().left())).unwrap();
    thumb.click_button(egui::PointerButton::Secondary);
    h.run_steps(2);
    h.get_all_by_label("Replace pages…").last().unwrap().click();
    h.run_steps(3);
    egui_kittest::kittest::Queryable::get_by_label(&h, "Replace Pages");
    assert_eq!(h.state().replace_draft.as_ref().unwrap().from, 2);
    assert_eq!(h.state().replace_draft.as_ref().unwrap().to, 2);
    egui_kittest::kittest::Queryable::get_by_label(&h, "OK").click();
    h.run_steps(3);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(doc.info.pages.len(), 4);
    assert_eq!(doc.can_undo(), Some("Replace page"));
    let _ = std::fs::remove_dir_all(&dir);
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
fn plus_between_pages_inserts_picked_files_there_in_order() {
    let dir = temp_path("insert-gap");
    std::fs::create_dir_all(&dir).unwrap();
    let (extra, note) = (dir.join("extra.pdf"), dir.join("note.txt"));
    std::fs::write(&extra, fixture(2)).unwrap();
    std::fs::write(&note, "a note").unwrap();
    let mut h = organize(3);
    // Page 3 is selected, yet the files go where the "+" is: between pages 1 and 2.
    h.get_by_label("Page 3").click();
    h.run_steps(2);
    h.state_mut().pick_override = Some(vec![extra.to_string_lossy().into_owned(), note.to_string_lossy().into_owned()]);
    h.get_by_label("Insert a file before page 2").click();
    h.run_steps(4);
    assert_eq!(texts_of(h.state(), 0), ["Page 1", "Page 1", "Page 2", "a note", "Page 2", "Page 3"]);
    assert_eq!(picked(&h), [1, 2, 3], "the inserted pages are selected");
    // Each file is one undo step.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert_eq!(texts_of(h.state(), 0), ["Page 1", "Page 2", "Page 3"]);
    // Both ends.
    h.state_mut().pick_override = Some(vec![note.to_string_lossy().into_owned()]);
    h.get_by_label("Insert a file before page 1").click();
    h.run_steps(4);
    h.get_by_label("Insert a file at the end").click();
    h.run_steps(4);
    assert_eq!(texts_of(h.state(), 0), ["a note", "Page 1", "Page 2", "Page 3", "a note"]);
    // The toolbar button still inserts after the selection, not at the last "+" used.
    h.get_by_label("Page 2").click();
    h.run_steps(2);
    insert_menu(&mut h, "Insert pages from a file…");
    h.run_steps(4);
    assert_eq!(texts_of(h.state(), 0), ["a note", "Page 1", "a note", "Page 2", "Page 3", "a note"]);
    // A file that can't be converted changes nothing.
    let before = texts_of(h.state(), 0);
    h.state_mut().use_files(pdfcraft_ui_egui::FilePurpose::InsertPages, vec![("report.docx".into(), b"PK\x03\x04".to_vec())]);
    h.run_steps(2);
    assert_eq!(texts_of(h.state(), 0), before);
}

/// A file dropped on the window, as the windowing layer hands it over.
#[derive(Debug)]
struct Dropped(&'static str, Vec<u8>);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &std::path::Path {
        std::path::Path::new(self.0)
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        Ok(self.1.clone())
    }
}

fn drop_files(h: &mut Harness<'static, PdfCraftApp>, files: Vec<Dropped>) {
    for f in files {
        h.input_mut().dropped_files.push(std::sync::Arc::new(f));
    }
}

#[test]
fn files_dropped_between_pages_are_inserted_there() {
    let mut h = organize(3);
    // Over the gap between pages 1 and 2.
    let gap = h.get_by_label("Insert a file before page 2").rect().center();
    h.hover_at(gap + egui::vec2(0.0, 40.0));
    h.run_steps(2);
    drop_files(&mut h, vec![Dropped("extra.pdf", fixture(2)), Dropped("note.txt", b"a note".to_vec())]);
    h.run_steps(4);
    assert_eq!(h.state().views.len(), 1, "dropped files are not opened as documents");
    assert_eq!(texts_of(h.state(), 0), ["Page 1", "Page 1", "Page 2", "a note", "Page 2", "Page 3"]);
    assert_eq!(picked(&h), [1, 2, 3], "the inserted pages are selected");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert_eq!(texts_of(h.state(), 0), ["Page 1", "Page 2", "Page 3"]);
    // Right of the last page: the end.
    let last = h.get_by_label("Page 3").rect();
    h.hover_at(last.center() + egui::vec2(last.width() * 0.4, 0.0));
    h.run_steps(2);
    drop_files(&mut h, vec![Dropped("note.txt", b"a note".to_vec())]);
    h.run_steps(4);
    assert_eq!(texts_of(h.state(), 0), ["Page 1", "Page 2", "Page 3", "a note"]);
}

#[test]
fn files_dropped_before_the_pointer_is_known_wait_for_it() {
    // Some platforms don't report the pointer while files are dragged over the window.
    let mut h = organize(2);
    drop_files(&mut h, vec![Dropped("note.txt", b"a note".to_vec())]);
    h.run_steps(1);
    assert_eq!(texts_of(h.state(), 0).len(), 2, "not placed yet");
    let gap = h.get_by_label("Insert a file before page 2").rect().center();
    h.hover_at(gap + egui::vec2(0.0, 40.0));
    h.run_steps(3);
    assert_eq!(texts_of(h.state(), 0), ["Page 1", "a note", "Page 2"]);
}

#[test]
fn files_dropped_outside_the_page_grid_still_open_as_documents() {
    let mut h = harness(2, |_| {});
    let original = h.state().views[0].id;
    drop_files(&mut h, vec![Dropped("other.pdf", fixture(1))]);
    support::wait_until(&mut h, "dropped document", |app| app.views.len() == 2 && app.password_prompt.is_none());
    assert_eq!(h.state().views.len(), 2);
    assert_eq!(texts_of(h.state(), 0), ["Page 1", "Page 2"]);
    assert!(h.state().views.iter().any(|view| view.id == original));
    assert_eq!(h.state().session.get(original).unwrap().bytes.as_slice(), fixture(2).as_slice());
    let (_, id) = h.state().active_ids().unwrap();
    let doc = h.state().session.get(id).unwrap();
    assert_eq!(doc.name, "other.pdf");
    assert_eq!(doc.info.pages.len(), 1);
    assert_eq!(doc.bytes.as_slice(), fixture(1).as_slice());
}

/// The Pages panel's thumbnail of page `label` (the panel is right of the page view).
fn panel_thumb_rect(h: &Harness<'static, PdfCraftApp>, label: &str) -> egui::Rect {
    h.get_all_by_label(label).map(|n| n.rect()).max_by(|a, b| a.left().total_cmp(&b.left())).unwrap()
}

#[test]
fn files_dropped_on_the_pages_panel_are_inserted_there() {
    let mut h = harness(3, |app| app.right = Some(pdfcraft_ui_egui::RightPanel::Pages));
    // The top half of page 2's thumbnail: between pages 1 and 2.
    let thumb = panel_thumb_rect(&h, "Page 2");
    h.hover_at(thumb.center() - egui::vec2(0.0, thumb.height() * 0.25));
    h.run_steps(2);
    assert_eq!(h.state().views[0].panel_gap, Some(1));
    drop_files(&mut h, vec![Dropped("extra.pdf", fixture(2)), Dropped("note.txt", b"a note".to_vec())]);
    h.run_steps(4);
    assert_eq!(h.state().views.len(), 1, "dropped files are not opened as documents");
    assert_eq!(texts_of(h.state(), 0), ["Page 1", "Page 1", "Page 2", "a note", "Page 2", "Page 3"]);
    assert_eq!(picked(&h), [1, 2, 3], "the inserted pages are selected");
    // The bottom half of the last thumbnail: the end.
    let mut h = harness(2, |app| app.right = Some(pdfcraft_ui_egui::RightPanel::Pages));
    let last = panel_thumb_rect(&h, "Page 2");
    h.hover_at(last.center() + egui::vec2(0.0, last.height() * 0.25));
    h.run_steps(2);
    drop_files(&mut h, vec![Dropped("note.txt", b"a note".to_vec())]);
    h.run_steps(4);
    assert_eq!(h.state().views.len(), 1);
    assert_eq!(texts_of(h.state(), 0), ["Page 1", "Page 2", "a note"]);
}

#[test]
fn files_dropped_beside_the_pages_panel_still_open_as_documents() {
    let mut h = harness(2, |app| app.right = Some(pdfcraft_ui_egui::RightPanel::Pages));
    // Over the page view, not the panel.
    let thumb = panel_thumb_rect(&h, "Page 1");
    h.hover_at(egui::pos2(thumb.left() - 300.0, thumb.center().y));
    h.run_steps(2);
    drop_files(&mut h, vec![Dropped("other.pdf", fixture(1))]);
    // Native opening prepares the document on a worker; three UI frames need not give it
    // any CPU time. Wait for the result, as the other dropped-document regression does.
    support::wait_until(&mut h, "document dropped beside the Pages panel", |app| app.views.len() == 2 && app.password_prompt.is_none());
    assert_eq!(h.state().views.len(), 2);
    assert_eq!(texts_of(h.state(), 0), ["Page 1", "Page 2"]);
    assert_eq!(texts_of(h.state(), 1), ["Page 1"], "the dropped PDF opens as its own document");
}

fn grid_zoom(h: &Harness<'static, PdfCraftApp>) -> f32 {
    h.state().views[0].grid_zoom()
}

#[test]
fn page_grid_zooms_with_its_buttons_keys_and_pinch() {
    let mut h = organize(12);
    let width = |h: &Harness<'static, PdfCraftApp>| h.get_by_label("Page 2").rect().left() - h.get_by_label("Page 1").rect().left();
    let usual = width(&h);
    h.get_by_label("Larger pages").click();
    h.run_steps(3);
    assert_eq!(grid_zoom(&h), 1.25);
    assert!(width(&h) > usual + 20.0, "the pages are drawn larger");
    h.get_by_label("Reset page size").click();
    h.run_steps(3);
    assert_eq!((grid_zoom(&h), width(&h)), (1.0, usual));
    h.get_by_label("Smaller pages").click();
    h.run_steps(3);
    assert_eq!(grid_zoom(&h), 0.8);
    assert!(width(&h) < usual - 20.0);
    // Keys.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num0);
    h.run_steps(2);
    assert_eq!(grid_zoom(&h), 1.0);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Equals);
    h.run_steps(2);
    assert_eq!(grid_zoom(&h), 1.25);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Minus);
    h.run_steps(2);
    assert_eq!(grid_zoom(&h), 1.0);
    // Pinch (or Ctrl/⌘ + wheel) over the grid; elsewhere it leaves the grid alone.
    let page = h.get_by_label("Page 1").rect().center();
    h.hover_at(page);
    h.run_steps(1);
    h.event(egui::Event::Zoom(1.5));
    h.run_steps(2);
    assert_eq!(grid_zoom(&h), 1.5);
    h.hover_at(h.get_by_label("Larger pages").rect().center());
    h.run_steps(1);
    h.event(egui::Event::Zoom(1.5));
    h.run_steps(2);
    assert_eq!(grid_zoom(&h), 1.5, "the pointer was on the toolbar");
    // It stops at its limits, where the buttons switch off.
    for _ in 0..12 {
        h.key_press_modifiers(Modifiers::COMMAND, Key::Equals);
        h.run_steps(1);
    }
    assert_eq!(grid_zoom(&h), 3.0);
    h.get_by_label("Larger pages").click();
    h.run_steps(2);
    assert_eq!(grid_zoom(&h), 3.0);
    for _ in 0..20 {
        h.key_press_modifiers(Modifiers::COMMAND, Key::Minus);
        h.run_steps(1);
    }
    assert_eq!(grid_zoom(&h), 0.5);
    assert!(!dirty(&h), "zooming is not an edit");
}

#[test]
fn zoomed_pages_in_view_are_rendered_at_the_size_drawn() {
    // A long document: sharpness must not depend on how many pages there are.
    let mut h = harness(400, |app| {
        app.set_option("organize", "on").unwrap();
        app.set_option("grid-zoom", "300").unwrap();
    });
    // The 200 × 300 fixture fits by height; keep its aspect ratio instead of stretching
    // it to the full cell width. Request enough pixels for that actual preview width.
    let drawn = 194.0 * 3.0 * (200.0 / 300.0) * h.ctx.pixels_per_point();
    let sharp = |h: &Harness<'static, PdfCraftApp>, page| h.state().views[0].grid_page_pixels(page);
    for _ in 0..200 {
        if sharp(&h, 0).is_some_and(|w| w as f32 >= drawn * 0.9) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        h.run_steps(1);
    }
    let width = sharp(&h, 0).expect("the first page has a sharp render");
    assert!((drawn * 0.9..=drawn * 1.6).contains(&(width as f32)), "{width} for {drawn}");
    assert!(sharp(&h, 399).is_none(), "pages out of view have none");
    // Zoomed out again, thumbnails are enough and the sharp renders are dropped.
    h.state_mut().set_option("grid-zoom", "50").unwrap();
    h.run_steps(3);
    assert!(sharp(&h, 0).is_none(), "page 0 still has a sharp render: {:?}", sharp(&h, 0));
}

#[test]
fn a_zoomed_page_grid_still_inserts_and_moves_at_the_right_gap() {
    let dir = temp_path("zoomed-grid");
    std::fs::create_dir_all(&dir).unwrap();
    let note = dir.join("note.txt");
    std::fs::write(&note, "a note").unwrap();
    let mut h = harness(3, |app| {
        app.set_option("organize", "on").unwrap();
        app.set_option("grid-zoom", "200").unwrap();
        assert!(
            app.set_option("grid-zoom", "20").is_err() && app.set_option("grid-zoom", "NaN").is_err() && app.set_option("grid-zoom", "big").is_err()
        );
    });
    assert_eq!(grid_zoom(&h), 2.0);
    h.state_mut().pick_override = Some(vec![note.to_string_lossy().into_owned()]);
    h.get_by_label("Insert a file before page 2").click();
    h.run_steps(4);
    assert_eq!(texts_of(h.state(), 0), ["Page 1", "a note", "Page 2", "Page 3"]);
    // Files dropped over a gap go there too.
    let gap = h.get_by_label("Insert a file before page 2").rect().center();
    h.hover_at(gap + egui::vec2(0.0, 60.0));
    h.run_steps(2);
    drop_files(&mut h, vec![Dropped("more.txt", b"more".to_vec())]);
    h.run_steps(4);
    assert_eq!(texts_of(h.state(), 0), ["Page 1", "more", "a note", "Page 2", "Page 3"]);
}

#[test]
fn save_pages_writes_what_the_grid_shows() {
    let path = temp_path("grid-save.pdf");
    let mut h = organize(3);
    h.get_by_label("Page 2").click();
    h.run_steps(2);
    h.key_press(Key::Delete);
    h.run_steps(3);
    h.state_mut().save_override = Some(path.to_string_lossy().into_owned());
    h.get_by_label("Save pages").click();
    h.run_steps(4);
    assert!(!dirty(&h));
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    app.open_bytes("saved.pdf", None, std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(texts_of(&app, 0), ["Page 1", "Page 3"]);
}

#[test]
fn split_dialog_writes_one_file_per_part() {
    let dir = temp_path("split-count");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let d = dir.to_string_lossy().into_owned();
    let mut h = organize(3);
    h.state_mut().export_dir_override = Some(d);
    h.get_all_by_label("Split").last().unwrap().click();
    h.run_steps(3);
    h.get_by_label_contains("Creates 3 files from 3 pages");
    h.get_all_by_label("Split").last().unwrap().click();
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
fn split_at_bookmarks_with_equal_titles_keeps_every_part() {
    let dir = temp_path("split-dup-titles");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = organize(3);
    h.state_mut().export_dir_override = Some(dir.to_string_lossy().into_owned());
    for (index, title) in [(0, "Same"), (1, "Same"), (2, "same")] {
        h.state_mut().apply_edit(pdfcraft_engine::Edit::AddBookmark { parent: vec![], index, title: title.into(), page: index });
    }
    h.state_mut().split_draft.mode = pdfcraft_ui_egui::SplitMode::Bookmarks;
    h.state_mut().run_command("page.split");
    h.run_steps(3);
    h.get_by_label_contains("Creates 3 files from 3 pages");
    h.get_all_by_label("Split").last().unwrap().click();
    h.run_steps(3);
    let mut names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, ["doc - Same (2).pdf", "doc - Same.pdf", "doc - same (3).pdf"]);
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
    h.get_all_by_label("Split").last().unwrap().click();
    h.run_steps(3);
    let mut names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, ["doc (pages 1-2).pdf", "doc (pages 3-4).pdf"]);
    let _ = std::fs::remove_dir_all(dir);
}

// ── Encrypted documents ───────────────────────────────────────────────────────────────────────

fn protected(user: &str, owner: &str, permissions: i32) -> Vec<u8> {
    protected_with(pdfcraft_cos::Algorithm::Aes256, user, owner, permissions)
}

fn protected_with(algorithm: pdfcraft_cos::Algorithm, user: &str, owner: &str, permissions: i32) -> Vec<u8> {
    let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(fixture(2))).unwrap();
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm,
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
        app.set_option("language", "en").unwrap();
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
    support::wait_until(&mut h, "unlocked document", |app| app.views.len() == 1);
    assert_eq!(h.state().views.len(), 1);
    h.state_mut().set_option("dialog", "properties").unwrap();
    h.run_steps(2);
    h.get_by_label("Security").click();
    h.run_steps(3);
    h.get_by_label("AES, 256-bit");
    h.get_by_label("User password");
}

/// #785: the notice's message used to be laid out before its buttons, so in a narrow document
/// area (a side panel open) it ran under "Security settings" and on into the panel, and it sat
/// off the buttons' centre line.
#[test]
fn the_security_notice_wraps_beside_its_buttons_and_lines_up_with_them() {
    // Wide: one line. Narrower: wrapped. Narrowest: cut short (the full text is on hover).
    for (width, wraps) in [(1700.0, false), (1250.0, true), (900.0, false)] {
        let mut h = Harness::builder().with_size(egui::vec2(width, 800.0)).build_eframe(|_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.open_bytes("locked.pdf", None, protected("", "owner", 0b0100)).unwrap();
            app.set_option("panel", "bookmarks").unwrap();
            app
        });
        h.run_steps(4);
        let msg = h.get_by_label_contains("This document is secured").rect();
        let button = h.get_by_label("Security settings").rect();
        let dismiss = h.get_by_label("Dismiss").rect();
        assert!(msg.right() <= button.left() + 0.5, "{width}: the message stops before the buttons: {msg:?} vs {button:?}");
        assert!(button.right() <= dismiss.left() + 0.5, "{width}: {button:?} vs {dismiss:?}");
        assert!((button.center().y - dismiss.center().y).abs() <= 0.5, "{width}: the buttons share a centre line");
        assert!(msg.top() >= button.top(), "{width}: the message stays inside the bar: {msg:?} vs {button:?}");
        let lines = (msg.height() / 14.0).floor().max(1.0);
        let first_line = msg.top() + msg.height() / lines / 2.0;
        assert!((first_line - button.center().y).abs() <= 1.5, "{width}: first line centred on the buttons: {msg:?} vs {button:?}");
        assert_eq!(msg.height() > button.height(), wraps, "{width}: {msg:?}");
    }
}

/// #995, #1077: a file with only a permissions password opens without asking, restricted. Its
/// notice offers Unlock…, which takes the permissions password and lifts the restrictions.
#[test]
fn the_security_notice_unlocks_with_the_permissions_password() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("locked.pdf", None, protected("", "owner", 0b0100)).unwrap();
        app
    });
    h.run_steps(4);
    h.get_by_label_contains("This document is secured");
    h.get_by_label("Unlock…").click();
    h.run_steps(2);
    h.get_by_label_contains("Enter its permissions password");
    type_password(&mut h, "wrong");
    h.get_by_label("Incorrect password. Try again.");
    type_password(&mut h, "owner");
    assert!(h.state().unlock_prompt.is_none());
    assert!(h.query_by_label_contains("This document is secured").is_none(), "the restrictions are lifted");
    h.state_mut().set_option("dialog", "properties").unwrap();
    h.run_steps(2);
    h.get_by_label("Security").click();
    h.run_steps(3);
    h.get_by_label("Owner password (no restrictions apply)");
    assert!(!h.state().session.get(h.state().active_ids().unwrap().1).unwrap().dirty, "unlocking is not an edit");
}

/// Changing a restricted document's security asks for its permissions password first (Acrobat
/// does too), then carries on.
#[test]
fn remove_security_asks_for_the_permissions_password_first() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("locked.pdf", None, protected("", "owner", 0b0100)).unwrap();
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("protect.remove"));
    h.run_steps(2);
    assert!(h.state().unlock_prompt.is_some(), "asks for the password");
    // Cancelling changes nothing.
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(h.state().unlock_prompt.is_none());
    let id = h.state().active_ids().unwrap().1;
    assert!(!h.state().session.get(id).unwrap().dirty);
    assert!(h.state_mut().execute("protect.remove"));
    h.run_steps(2);
    type_password(&mut h, "owner");
    let doc = h.state().session.get(id).unwrap();
    assert!(doc.dirty && doc.security_summary().is_none(), "security is removed (written by the next save)");
}

#[test]
fn restricted_documents_show_a_notice_and_block_page_changes() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
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
    assert_eq!(h.query_all_by_label_contains("Insert a file").count(), 0, "nothing to insert into");
    // Looking closer is not a page change.
    h.get_by_label("Larger pages").click();
    h.run_steps(2);
    assert_eq!(grid_zoom(&h), 1.25);
    assert!(!dirty(&h));
    h.get_by_label("Security settings").click();
    h.run_steps(3);
    assert!(h.query_all_by_label("Not allowed").count() >= 4);
}

/// Table 22, bit 5 (#568): a document whose security withholds copying keeps its text and images
/// in. ⌘C, snapshots and exports are refused with a notice; the owner password lifts that.
#[test]
fn copying_out_of_a_document_honours_its_security() {
    fn open(password: &'static str) -> Harness<'static, PdfCraftApp> {
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.system_clipboard = false;
            app.open_bytes("no-copy.pdf", None, protected("pw", "owner", -1 ^ 0b1_0000)).unwrap();
            app.submit_password(Some(password.into()));
            app
        });
        // Select the page's text once its text layer is loaded (up to ~4 s: a loaded test machine
        // can take far longer than the 0.2 s this used to allow).
        for _ in 0..200 {
            h.run_steps(2);
            if h.state_mut().views[0].select_all() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(h.state().views[0].selected_text().as_deref(), Some("Page 1"));
        h
    }
    fn copy(h: &mut Harness<'static, PdfCraftApp>) -> bool {
        h.state_mut().toast = None;
        h.event(egui::Event::Copy);
        let mut copied = false;
        for _ in 0..4 {
            h.step();
            copied |= h.output().platform_output.commands.iter().any(|c| matches!(c, egui::OutputCommand::CopyText(t) if t == "Page 1"));
        }
        copied
    }
    fn snapshot(h: &mut Harness<'static, PdfCraftApp>) {
        // Around "Page 1", at (20, 150) on the 200 × 300 pt page.
        let r = h.state().views[0].page_screen_rect(0).expect("on screen");
        let at = |x: f32, y: f32| egui::pos2(r.left() + x / 200.0 * r.width(), r.top() + (300.0 - y) / 300.0 * r.height());
        let (a, b) = (at(15.0, 175.0), at(120.0, 140.0));
        h.hover_at(a);
        h.run_steps(1);
        h.drag_at(a);
        h.run_steps(1);
        h.hover_at(b);
        h.run_steps(1);
        h.drop_at(b);
        h.run_steps(3);
    }
    let refused = Some("The document's security settings don't allow copying text and images");

    let mut h = open("pw");
    assert!(!copy(&mut h), "⌘C copies nothing");
    assert_eq!(h.state().toast.as_ref().map(|t| t.0.as_str()), refused);
    for command in ["edit.snapshot", "export.image", "export.text", "export.docx", "export.pptx", "export.html", "export.rtf", "export.all_images"] {
        h.state_mut().toast = None;
        assert!(!h.state_mut().execute(command), "{command} is refused");
        assert_eq!(h.state().toast.as_ref().map(|t| t.0.as_str()), refused, "{command} says why");
    }
    assert_eq!(h.state().dialog, None, "no export dialog");
    // The snapshot tool, chosen on another tab, takes nothing here either.
    h.state_mut().set_option("quick", "snapshot").unwrap();
    snapshot(&mut h);
    assert!(h.state().last_snapshot.is_none());
    assert_eq!(h.state().toast.as_ref().map(|t| t.0.as_str()), refused);

    let mut h = open("owner");
    assert!(copy(&mut h), "the owner may copy");
    assert!(h.state_mut().execute("edit.snapshot"));
    snapshot(&mut h);
    assert!(h.state().last_snapshot.is_some());
}

#[test]
fn replace_pages_dialog_swaps_page_content() {
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
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

/// Pages 1 and 3 are selected. OK replaces those two and leaves the unchecked page between them.
#[test]
fn replace_split_selection_keeps_the_unchecked_page() {
    fn sized(n: usize, width: u32, height: u32, label: &str) -> Vec<u8> {
        let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into()];
        let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
        objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 {width} {height}] >>", kids.join(" ")));
        objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
        for i in 0..n {
            objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * i));
            let body = format!("BT /F1 24 Tf 20 150 Td ({label} {}) Tj ET", i + 1);
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
    let open = |selected: &[usize], edited: bool| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("original.pdf", None, sized(3, 200, 300, "Original")).unwrap();
        app.views[0].select_pages(selected);
        app.start_replace("other.pdf".into(), sized(5, 400, 500, "Replacement"));
        if edited {
            app.replace_draft.as_mut().unwrap().range_edited = true;
        }
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| app);
        h.run_steps(2);
        h
    };
    let sizes = |app: &PdfCraftApp| {
        let doc = app.session.get(app.views[0].id).unwrap();
        doc.info.pages.iter().map(|p| (p.width.round() as i32, p.height.round() as i32)).collect::<Vec<_>>()
    };
    let mut split = open(&[0, 2], false);
    let draft = split.state().replace_draft.as_ref().unwrap();
    assert_eq!((draft.from, draft.to, draft.pages.as_slice(), draft.range_edited), (1, 3, &[0usize, 2][..], false));
    egui_kittest::kittest::Queryable::get_by_label(&split, "OK").click();
    split.run_steps(3);
    assert_eq!(texts_of(split.state(), 0), ["Replacement 1", "Original 2", "Replacement 2"]);
    assert_eq!(sizes(split.state()), [(400, 500), (200, 300), (400, 500)]);
    assert_eq!(split.state().session.get(split.state().views[0].id).unwrap().can_undo(), Some("Replace pages"));
    split.state_mut().undo();
    split.run_steps(2);
    assert_eq!(texts_of(split.state(), 0), ["Original 1", "Original 2", "Original 3"]);
    assert_eq!(sizes(split.state()), [(200, 300), (200, 300), (200, 300)]);

    let mut span = open(&[0, 2], true);
    egui_kittest::kittest::Queryable::get_by_label(&span, "OK").click();
    span.run_steps(3);
    assert_eq!(texts_of(span.state(), 0), ["Replacement 1", "Replacement 2", "Replacement 3"]);

    let mut adjacent = open(&[1, 2], false);
    egui_kittest::kittest::Queryable::get_by_label(&adjacent, "OK").click();
    adjacent.run_steps(3);
    assert_eq!(texts_of(adjacent.state(), 0), ["Original 1", "Replacement 1", "Replacement 2"]);
    assert_eq!(sizes(adjacent.state()), [(200, 300), (400, 500), (400, 500)]);
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
    h.state_mut().extract_draft = pdfcraft_ui_egui::ExtractDraft { separate: true, delete: true, ..Default::default() };
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
    source_font_fixture_with_text("BT /F1 18 Tf 20 220 Td (Serif) Tj ET BT /F2 18 Tf 20 120 Td (Mono) Tj ET")
}

fn source_font_fixture_with_text(body: &str) -> Vec<u8> {
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
        app.set_option("language", "en").unwrap();
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
fn deleting_a_draft_does_not_show_the_original_text_underneath() {
    let mut h = harness(1, |_| {});
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(2);
    let r = h.state().views[0].page_screen_rect(0).unwrap();
    let at = egui::pos2(r.left() + 40.0 / 200.0 * r.width(), r.top() + 142.0 / 300.0 * r.height());
    h.hover_at(at);
    h.run_steps(1);
    h.drag_at(at);
    h.run_steps(1);
    h.drop_at(at);
    h.run_steps(3);
    h.state_mut().views[0].line_editor.as_mut().unwrap().text.clear();
    for _ in 0..100 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let shot = h.render().unwrap();
    if let Ok(path) = std::env::var("PDFCRAFT_TEXT_PREVIEW_SHOT") {
        shot.save(path).unwrap();
    }
    // The right half of the original glyphs, away from the caret and editor border.
    let k = r.width() / 200.0;
    for y in 152..166 {
        for x in 65..82 {
            let px = (r.left() + x as f32 * k) as u32;
            let py = (r.top() + (300 - y) as f32 * k) as u32;
            let [red, green, blue, alpha] = shot.get_pixel(px, py).0;
            assert!(
                !(red < 100 && green < 100 && blue < 100) && alpha == 255,
                "the deleted source is still visible at {x},{y}: {:?}",
                [red, green, blue, alpha]
            );
        }
    }
    assert!(!dirty(&h), "the document is unchanged until the draft is applied");
    assert_eq!(page_texts(h.state()), ["Page 1"]);
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert_eq!(page_texts(h.state()), ["Page 1"]);
}

#[test]
fn switching_from_a_narrow_editor_keeps_one_frame_and_a_wide_heading_on_one_line() {
    let bytes = source_font_fixture_with_text(
        "BT /F1 18 Tf 20 220 Td (Editable PDF heading) Tj ET BT /F2 18 Tf 20 120 Td (Mono) Tj ET BT /F2 18 Tf 20 98 Td (Text) Tj ET",
    );
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("editor.pdf", None, bytes).unwrap();
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(2);
    let page = h.state().views[0].page_screen_rect(0).unwrap();
    for y in [128.0, 228.0] {
        let point = egui::pos2(page.left() + 25.0 / 300.0 * page.width(), page.top() + (300.0 - y) / 300.0 * page.height());
        h.hover_at(point);
        h.run_steps(1);
        h.drag_at(point);
        h.run_steps(1);
        h.drop_at(point);
        h.run_steps(4);
        if y == 128.0 {
            assert_eq!(h.state().views[0].line_editor.as_ref().unwrap().text, "Mono\nText", "opening a paragraph keeps its source line breaks");
            h.key_press(Key::Escape);
            h.run_steps(2);
        }
    }
    let heading = h
        .output()
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == "Editable PDF heading" => Some(&text.galley),
            _ => None,
        })
        .expect("the heading editor is painted");
    assert_eq!(heading.rows.len(), 1, "opening a wider block must not reuse the previous editor's narrow width");
    let frames = h
        .output()
        .shapes
        .iter()
        .filter(|shape| {
            matches!(&shape.shape,
                egui::Shape::Rect(rect) if rect.stroke.color == egui::Color32::from_rgb(0x14, 0x73, 0xE6)
                    && rect.rect.width() > 100.0 && page.contains(rect.rect.center())
            )
        })
        .count();
    assert_eq!(frames, 1, "the active block has only the editor frame");
    if let Ok(path) = std::env::var("PDFCRAFT_TEXT_FRAME_SHOT") {
        h.render().unwrap().save(path).unwrap();
    }
    assert!(!dirty(&h));
}

/// #2022: clicking straight into another text box while one is open for typing must apply the
/// open paragraph first; the typed text used to be dropped with the editor that switched away.
#[test]
fn switching_straight_to_another_box_applies_the_open_paragraph() {
    let bytes = source_font_fixture_with_text("BT /F1 24 Tf 20 220 Td (Top) Tj ET BT /F2 24 Tf 20 120 Td (Bottom) Tj ET");
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("boxes.pdf", None, bytes).expect("opens");
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(2);
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let k = r.width() / 300.0;
    let mut blocks = {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().text_blocks(0)
    };
    blocks.sort_by(|a, b| b.rect[1].partial_cmp(&a.rect[1]).unwrap_or(std::cmp::Ordering::Equal));
    assert_eq!(blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["Top", "Bottom"], "two editable boxes");
    let center = |b: &pdfcraft_engine::TextBlock| {
        egui::pos2(r.left() + ((b.rect[0] + b.rect[2]) / 2.0) as f32 * k, r.top() + (300.0 - (b.rect[1] + b.rect[3]) / 2.0) as f32 * k)
    };
    // Open the top box and type into it.
    click_at(&mut h, center(&blocks[0]));
    let ed = h.state().views[0].line_editor.clone().expect("the top box opens for typing");
    assert_eq!(ed.text, "Top");
    h.state_mut().views[0].line_editor.as_mut().unwrap().text = "Top edited".into();
    h.run_steps(1);
    // Click straight into the bottom box: the open paragraph is applied, and the bottom box opens.
    click_at(&mut h, center(&blocks[1]));
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(
        doc.text_blocks(0).iter().map(|b| b.text.clone()).collect::<Vec<_>>(),
        ["Top edited", "Bottom"],
        "the typed text is applied, not dropped"
    );
    assert_eq!(doc.can_undo(), Some("Edit text"));
    let ed = s.views[0].line_editor.clone().expect("the bottom box is now open");
    assert_eq!(ed.text, "Bottom");
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
    // The source container stays visible while added text can overflow it.
    assert!(ed.growth().is_none(), "opening the editor preserves the source container");
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

#[test]
fn text_tool_clicking_source_text_edits_instead_of_adding_a_copy() {
    let mut h = harness(1, |_| {});
    assert!(h.state_mut().execute("edit.text"));
    h.run_steps(2);
    let page = h.state().views[0].page_screen_rect(0).unwrap();
    let blank = egui::pos2(page.left() + 0.2 * page.width(), page.top() + 0.1 * page.height());
    h.hover_at(blank);
    h.run_steps(1);
    h.drag_at(blank);
    h.run_steps(1);
    h.drop_at(blank);
    h.run_steps(3);
    h.event(egui::Event::Text("Keep this note".into()));
    h.run_steps(3);
    let point = egui::pos2(page.left() + 40.0 / 200.0 * page.width(), page.top() + 142.0 / 300.0 * page.height());
    h.hover_at(point);
    h.run_steps(1);
    h.drag_at(point);
    h.run_steps(1);
    h.drop_at(point);
    h.run_steps(4);
    assert!(h.state().views[0].content.draft.is_none(), "clicking source text must not start an added layer");
    assert_eq!(h.state().views[0].line_editor.as_ref().unwrap().text, "Page 1");
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().added().len(), 1, "the previous draft is kept");
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.event(egui::Event::Text("Replacement".into()));
    h.run_steps(3);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
    h.run_steps(3);
    assert_eq!(page_texts(h.state()), ["Keep this note\nReplacement"]);
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().added().len(), 1, "editing the source adds no second layer");
}

#[test]
fn text_tool_mixed_style_row_grows_in_the_editor_and_saved_pdf() {
    let bytes = source_font_fixture_with_text("BT /F1 12 Tf 20 220 Td (Small ) Tj /F2 12 Tf (row) Tj ET");
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("mixed-row.pdf", None, bytes).unwrap();
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(2);
    let page = h.state().views[0].page_screen_rect(0).unwrap();
    let point = egui::pos2(page.left() + 25.0 / 300.0 * page.width(), page.top() + 75.0 / 300.0 * page.height());
    h.hover_at(point);
    h.run_steps(1);
    h.drag_at(point);
    h.run_steps(1);
    h.drop_at(point);
    h.run_steps(4);
    let replacement = "Small row with more words";
    h.key_press_modifiers(Modifiers::COMMAND, Key::End);
    h.event(egui::Event::Text(" with more words".into()));
    h.run_steps(4);
    let galley = h
        .output()
        .shapes
        .iter()
        .find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.text() == replacement => Some(&t.galley),
            _ => None,
        })
        .expect("the draft is painted");
    assert_eq!(galley.rows.len(), 1, "mixed source runs grow as one visual row");
    if let Ok(path) = std::env::var("PDFCRAFT_MIXED_ROW_SHOT") {
        h.render().unwrap().save(path).unwrap();
    }
    h.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
    h.run_steps(3);
    let state = h.state();
    let doc = state.session.get(state.views[0].id).unwrap();
    assert_eq!(doc.text_blocks(0)[0].editor_text, replacement);
    assert_eq!(doc.text_blocks(0)[0].rows.len(), 1);
    let bytes = state.session.save_bytes(state.views[0].id).unwrap();
    let mut reopened = pdfcraft_engine::Session::new();
    let id = reopened.open("again.pdf", None, bytes, None).unwrap();
    assert_eq!(reopened.get(id).unwrap().text_blocks(0)[0].rows.len(), 1);
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
        app.set_option("language", "en").unwrap();
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
    check_existing_artwork("picture.png", png);
}

#[test]
fn editing_existing_form_artwork_on_the_page() {
    use pdfcraft_cos::{Dict, Document, Object, SaveOptions, Stream, write_incremental};
    let mut doc = Document::open(std::sync::Arc::new(fixture(1))).unwrap();
    let page = pdfcraft_model::pages(&doc)[0].clone();
    let mut form = Dict::new();
    form.set(b"Subtype".to_vec(), Object::name("Form"));
    form.set(b"BBox".to_vec(), Object::Array([0, 0, 200, 300].map(Object::Int).to_vec()));
    let artwork = doc.add(Object::Stream(Stream::flate(form, b"0.2 0.5 0.9 rg 20 20 160 260 re f")));
    let contents = doc.add(Object::Stream(Stream::flate(Dict::new(), b"/Figure Do")));
    doc.update_dict(page.obj, |d| {
        d.set(b"Contents".to_vec(), Object::Ref(contents));
        let mut xo = Dict::new();
        xo.set(b"Figure".to_vec(), Object::Ref(artwork));
        let mut resources = Dict::new();
        resources.set(b"XObject".to_vec(), Object::Dict(xo));
        d.set(b"Resources".to_vec(), Object::Dict(resources));
    })
    .unwrap();
    check_existing_artwork("figure.pdf", write_incremental(&doc, &SaveOptions::default()).unwrap());
}

fn check_existing_artwork(name: &'static str, bytes: Vec<u8>) {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes(name, None, bytes.clone()).expect("opens");
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
    h.get_by_label("Bold").click();
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

/// Horizontal scale and Character spacing take any value in their ranges, not only the listed
/// ones: values between the listed ones and beyond them, typed in Format text, reach the saved
/// file as the engine writes them.
#[test]
fn the_format_panel_takes_any_scale_and_character_spacing() {
    let mut h = harness(1, |_| {});
    let id = h.state().views[0].id;
    let width = |h: &Harness<'static, PdfCraftApp>, view: usize| {
        let s = h.state();
        let r = s.session.get(s.views[view].id).unwrap().text_blocks(0)[0].rect;
        r[2] - r[0]
    };
    let plain = width(&h, 0);
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
    // Typed the way a person does: click the field, type, Enter.
    let type_in = |h: &mut Harness<'static, PdfCraftApp>, label: &str, value: &str| {
        h.get_by_role_and_label(Role::SpinButton, label).click();
        h.run_steps(2);
        h.event(egui::Event::Text(value.into()));
        h.run_steps(1);
        h.key_press(Key::Enter);
        h.run_steps(4);
    };
    let extras = |h: &Harness<'static, PdfCraftApp>| h.state().views[0].line_editor.as_ref().map(|e| (e.extras.scale, e.extras.char_spacing));
    // Between the listed values (50…200 %, −1…5 pt) and beyond them, within what was always allowed.
    type_in(&mut h, "Horizontal scale", "80");
    assert_eq!(extras(&h), Some((80.0, 0.0)));
    type_in(&mut h, "Horizontal scale", "250");
    assert_eq!(extras(&h), Some((250.0, 0.0)));
    type_in(&mut h, "Character spacing", "0.25");
    assert_eq!(extras(&h), Some((250.0, 0.25)));
    type_in(&mut h, "Character spacing", "-2");
    assert_eq!(extras(&h), Some((250.0, -2.0)));
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Edit text"));
    // Saved and reopened, the paragraph is as wide as the engine draws it with those values.
    let bytes = h.state().session.save_bytes(id).unwrap();
    h.state_mut().open_bytes("reference.pdf", None, fixture(1)).expect("opens");
    let style = pdfcraft_engine::BlockStyle { scale: Some(250.0), char_spacing: Some(-2.0), ..Default::default() };
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::EditTextBlock { page: 0, block: 0, text: "Page 1".into(), style }));
    h.state_mut().open_bytes("saved.pdf", None, bytes.as_ref().clone()).expect("the saved file opens");
    h.run_steps(3);
    let (reference, saved) = (width(&h, 1), width(&h, 2));
    assert!(saved > plain * 1.5, "250 % widens it: {plain} → {saved}");
    assert!((saved - reference).abs() < 0.5, "saved {saved}, the engine's own {reference}");
}

#[test]
fn selected_text_formatting_keeps_the_rest_of_the_paragraph_and_reopens() {
    let mut h = harness(1, |_| {});
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(2);
    let r = h.state().views[0].page_screen_rect(0).unwrap();
    let at = egui::pos2(r.left() + 40.0 / 200.0 * r.width(), r.top() + (300.0 - 158.0) / 300.0 * r.height());
    h.hover_at(at);
    h.run_steps(1);
    h.drag_at(at);
    h.run_steps(1);
    h.drop_at(at);
    h.run_steps(4);
    h.key_press_modifiers(Modifiers::COMMAND, Key::ArrowRight);
    h.run_steps(1);
    h.key_press_modifiers(Modifiers::SHIFT, Key::ArrowLeft);
    h.run_steps(2);
    assert_eq!(h.state().views[0].line_editor.as_ref().unwrap().selection, Some(5..6));
    h.get_by_label("Bold").click();
    h.run_steps(5);
    let id = h.state().views[0].id;
    let doc = h.state().session.get(id).unwrap();
    let lines = doc.text_lines(0);
    assert!(lines.iter().any(|l| l.text == "1" && l.base_font == "Helvetica-Bold"), "{lines:?}");
    assert!(lines.iter().any(|l| l.text.trim_end() == "Page" && l.base_font == "Helvetica"));
    let saved = h.state_mut().session.save_bytes(id).unwrap();
    let mut reopen = pdfcraft_engine::Session::new();
    let rid = reopen.open("saved.pdf", None, saved, None).unwrap();
    assert_eq!(reopen.get(rid).unwrap().text_lines(0), lines);
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
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Move object"));
    // Drag the handle on its right edge in to about 45 pt wide: "Page" and "1" rewrap onto two lines.
    let edge = screen(moved.rect[2], (moved.rect[1] + moved.rect[3]) / 2.0) + egui::vec2(2.0, 0.0);
    let narrower = (moved.rect[2] - moved.rect[0] - 45.0) as f32 * k;
    drag(&mut h, edge, edge - egui::vec2(narrower, 0.0));
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    let lines: Vec<String> = doc.text_lines(0).iter().map(|l| l.text.clone()).collect();
    assert_eq!(lines, ["Page", "1"], "rewrapped to the narrower box");
    assert!(near(doc.text_lines(0)[0].rect[0], moved.rect[0]), "it keeps its place");
    assert_eq!(doc.can_undo(), Some("Edit text"), "edge drags still rewrap text");
    assert!(h.state().views[0].line_editor.is_none());
}

/// Press and release at `at` without moving.
fn click_at(h: &mut Harness<'static, PdfCraftApp>, at: egui::Pos2) {
    h.hover_at(at);
    h.run_steps(1);
    h.drag_at(at);
    h.run_steps(1);
    h.drop_at(at);
    h.run_steps(4);
}

fn press(h: &mut Harness<'static, PdfCraftApp>, modifiers: Modifiers, key: Key) {
    h.key_press_modifiers(modifiers, key);
    h.run_steps(4);
}

#[test]
fn arrow_keys_nudge_a_selected_image_instead_of_turning_the_page() {
    use pdfcraft_cos::{Dict, Document, Object, SaveOptions, Stream, write_incremental};
    let mut doc = Document::open(std::sync::Arc::new(fixture(2))).unwrap();
    let page = pdfcraft_model::pages(&doc)[0].clone();
    let mut form = Dict::new();
    form.set(b"Subtype".to_vec(), Object::name("Form"));
    form.set(b"BBox".to_vec(), Object::Array([0, 0, 200, 300].map(Object::Int).to_vec()));
    let artwork = doc.add(Object::Stream(Stream::flate(form, b"0.2 0.5 0.9 rg 40 200 120 80 re f")));
    let contents = doc.add(Object::Stream(Stream::flate(Dict::new(), b"/Figure Do")));
    doc.update_dict(page.obj, |d| {
        d.set(b"Contents".to_vec(), Object::Ref(contents));
        let mut xo = Dict::new();
        xo.set(b"Figure".to_vec(), Object::Ref(artwork));
        let mut resources = Dict::new();
        resources.set(b"XObject".to_vec(), Object::Dict(xo));
        d.set(b"Resources".to_vec(), Object::Dict(resources));
    })
    .unwrap();
    let bytes = write_incremental(&doc, &SaveOptions::default()).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("figure.pdf", None, bytes.clone()).expect("opens");
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(2);
    let id = h.state().views[0].id;
    let rect = |h: &Harness<'static, PdfCraftApp>| h.state().session.get(id).unwrap().page_images(0)[0].rect;
    let before = rect(&h);
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    click_at(&mut h, egui::pos2(r.left() + 100.0 / 200.0 * r.width(), r.top() + (300.0 - 240.0) / 300.0 * r.height()));
    assert!(h.state().views[0].image_selection.is_some(), "selected");
    // As in Acrobat: an arrow moves it a point, Shift and an arrow ten.
    press(&mut h, Modifiers::NONE, Key::ArrowRight);
    press(&mut h, Modifiers::SHIFT, Key::ArrowDown);
    let moved = rect(&h);
    let near = |a: f64, b: f64| (a - b).abs() < 0.01;
    assert!(near(moved[0], before[0] + 1.0) && near(moved[1], before[1] - 10.0), "{before:?} → {moved:?}");
    assert!(near(moved[2] - moved[0], before[2] - before[0]), "moved, not resized");
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Move image"));
    assert_eq!(h.state().views[0].current, 0, "the arrows moved the image, not the page");
    assert!(h.state().views[0].image_selection.is_some(), "it stays selected for the next nudge");
}

#[test]
fn an_open_paragraph_moves_by_its_frame_and_by_the_arrow_keys() {
    let mut h = harness(2, |_| {});
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(2);
    let id = h.state().views[0].id;
    let block = |h: &Harness<'static, PdfCraftApp>| h.state().session.get(id).unwrap().text_blocks(0)[0].clone();
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let k = r.width() / 200.0;
    let screen = |x: f64, y: f64| egui::pos2(r.left() + x as f32 * k, r.top() + (300.0 - y as f32) * k);
    // Just above the paragraph's frame: on the frame, off its text.
    let frame = |b: &pdfcraft_engine::TextBlock| screen((b.rect[0] + b.rect[2]) / 2.0, b.rect[3]) - egui::vec2(0.0, 4.0);
    let near = |a: f64, b: f64| (a - b).abs() < 0.01;

    let before = block(&h);
    click_at(&mut h, screen((before.rect[0] + before.rect[2]) / 2.0, (before.rect[1] + before.rect[3]) / 2.0));
    assert!(h.state().views[0].line_editor.is_some(), "a click on the text opens it for typing");
    // A click on its frame selects the box instead, and the arrows move it.
    click_at(&mut h, frame(&before));
    assert!(h.state().views[0].line_editor.is_none(), "the frame selects the box, it doesn't type");
    press(&mut h, Modifiers::NONE, Key::ArrowRight);
    press(&mut h, Modifiers::SHIFT, Key::ArrowUp);
    let nudged = block(&h);
    assert_eq!(nudged.text, "Page 1");
    assert!(near(nudged.rect[0], before.rect[0] + 1.0) && near(nudged.rect[1], before.rect[1] + 10.0), "{:?} → {:?}", before.rect, nudged.rect);
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Move object"));
    assert_eq!(h.state().views[0].current, 0, "the arrows moved the paragraph, not the page");

    // Open it again and drag it by its frame: it follows the pointer.
    press(&mut h, Modifiers::NONE, Key::Escape);
    click_at(&mut h, screen((nudged.rect[0] + nudged.rect[2]) / 2.0, (nudged.rect[1] + nudged.rect[3]) / 2.0));
    assert!(h.state().views[0].line_editor.is_some());
    let from = frame(&nudged);
    drag(&mut h, from, from + egui::vec2(30.0 * k, -50.0 * k));
    assert!(h.state().views[0].line_editor.is_none());
    let dragged = block(&h);
    assert!(
        (dragged.rect[0] - nudged.rect[0] - 30.0).abs() < 1.5 && (dragged.rect[1] - nudged.rect[1] - 50.0).abs() < 1.5,
        "{:?} → {:?}",
        nudged.rect,
        dragged.rect
    );
    assert_eq!(h.state().session.get(id).unwrap().text_lines(0).len(), 1, "moved, not retyped");
}

/// Contributor-original text, with an off-origin CropBox. Keep the moving paragraph last in
/// source order so an ordinary hit picks it again when two independent boxes fully overlap.
fn paragraph_drag_fixture(content: &str, rotation: i64, unit: i64) -> Vec<u8> {
    use pdfcraft_cos::{Dict, Document, Object, Stream};
    let mut doc = Document::open(std::sync::Arc::new(fixture(1))).unwrap();
    let page = pdfcraft_model::pages(&doc)[0].obj;
    let contents = doc.add(Object::Stream(Stream::flate(Dict::new(), content.as_bytes())));
    doc.update_dict(page, |d| {
        d.set(b"Contents".to_vec(), Object::Ref(contents));
        d.set(b"MediaBox".to_vec(), Object::Array([0, 0, 400, 500].map(Object::Int).to_vec()));
        d.set(b"CropBox".to_vec(), Object::Array([20, 30, 380, 470].map(Object::Int).to_vec()));
        d.set(b"Rotate".to_vec(), Object::Int(rotation));
        d.set(b"UserUnit".to_vec(), Object::Int(unit));
    })
    .unwrap();
    pdfcraft_cos::write_incremental(&doc, &Default::default()).unwrap()
}

fn paragraph_drag_harness(bytes: Vec<u8>, view_rotation: u16) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1100.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.session = pdfcraft_engine::Session::new().with_clock(|| 1_700_000_000);
        app.set_option("language", "en").unwrap();
        app.open_bytes("paragraphs.pdf", None, bytes).unwrap();
        app.set_option("zoom", "75").unwrap();
        app.views[0].rotation = view_rotation;
        assert!(app.execute("edit.edit_text"));
        app
    });
    h.run_steps(4);
    h
}

fn dragged_paragraphs(h: &Harness<'static, PdfCraftApp>) -> Vec<pdfcraft_engine::TextBlock> {
    let app = h.state();
    app.session.get(app.views[0].id).unwrap().text_blocks(0)
}

/// Resolve the current inventory and transform for every gesture, never an old block index
/// or cached screen position. With identical labels, choose the last (topmost) source box.
fn paragraph_drag_points(h: &Harness<'static, PdfCraftApp>, text: &str, offset: [f64; 2]) -> (egui::Pos2, egui::Pos2) {
    let app = h.state();
    let view = &app.views[0];
    let doc = app.session.get(view.id).unwrap();
    let b = doc.text_blocks(0).into_iter().rfind(|b| b.text == text).unwrap();
    let xf = pdfcraft_ui_egui::canvas::PageXform {
        rect: view.page_screen_rect(0).unwrap(),
        rot: view.rotation,
        pw: doc.info.pages[0].width,
        ph: doc.info.pages[0].height,
    };
    let moved = std::array::from_fn(|i| (b.rect[i] + offset[i % 2]) as f32);
    (xf.user_rect(&doc.info, 0, b.rect.map(|v| v as f32)).center(), xf.user_rect(&doc.info, 0, moved).center())
}

fn drag_paragraph(h: &mut Harness<'static, PdfCraftApp>, text: &str, offset: [f64; 2]) {
    let (from, to) = paragraph_drag_points(h, text, offset);
    h.get_by_label("Shift-click to select several objects. Drag empty space to select an area.");
    assert!(h.state().views[0].line_editor.is_none());
    drag(h, from, to);
    assert!(h.state().views[0].line_editor.is_none(), "a body drag must not open the typing editor");
    h.get_by_label("Shift-click to select several objects. Drag empty space to select an area.");
}

fn assert_dragged_paragraphs(actual: &[pdfcraft_engine::TextBlock], expected: &[pdfcraft_engine::TextBlock]) {
    assert_eq!(actual.len(), expected.len(), "independent paragraphs must survive");
    // Source order is itself an invariant, including when text and geometry are identical.
    for (a, e) in actual.iter().zip(expected) {
        assert_eq!((&a.text, &a.base_font, a.size, a.bold, a.italic, a.color), (&e.text, &e.base_font, e.size, e.bold, e.italic, e.color));
        assert_eq!(a.lines.len(), e.lines.len(), "moving must not rewrap");
        assert!(a.rect.iter().zip(e.rect).all(|(a, e)| (a - e).abs() < 0.01), "{a:?} != {e:?}");
    }
}

/// Read only the current page contents, not stale revisions or the renderer's deduplicated
/// text layer. These synthetic fixtures contain each literal exactly once per source draw.
fn paragraph_drag_content(h: &Harness<'static, PdfCraftApp>) -> String {
    let app = h.state();
    let doc = pdfcraft_cos::Document::open(app.session.get(app.views[0].id).unwrap().bytes.clone()).unwrap();
    let page = pdfcraft_model::pages(&doc)[0].clone();
    let contents = doc.resolve(page.dict.get(b"Contents").unwrap());
    let pieces = match contents.as_array() {
        Some(a) => a.clone(),
        None => vec![contents.as_ref().clone()],
    };
    let mut bytes = Vec::new();
    for piece in pieces {
        let object = doc.resolve(&piece);
        let pdfcraft_cos::Object::Stream(stream) = object.as_ref() else { panic!("fixture content must be a stream") };
        bytes.extend(stream.decoded().unwrap());
    }
    String::from_utf8(bytes).unwrap()
}

fn save_paragraph_drag(h: &mut Harness<'static, PdfCraftApp>, name: &str, original: &[u8]) -> Vec<u8> {
    let out = temp_path(name);
    let id = h.state().views[0].id;
    h.state_mut().save_override = Some(out.to_string_lossy().into_owned());
    h.key_press_modifiers(Modifiers::COMMAND, Key::S);
    h.run_steps(4);
    let saved = std::fs::read(&out).unwrap();
    assert!(saved.starts_with(original), "Save keeps the opened/saved revisions");
    assert!(!h.state().session.get(id).unwrap().dirty);
    assert_eq!(h.state().session.get(id).unwrap().bytes.as_slice(), saved.as_slice());
    std::fs::remove_file(out).unwrap();
    saved
}

fn paragraph_drag_shot(h: &mut Harness<'static, PdfCraftApp>, name: &str) {
    if let Ok(directory) = std::env::var("PDFCRAFT_PARAGRAPH_DRAG_SHOTS") {
        for _ in 0..60 {
            h.run_steps(2);
            if !h.state().render_pending() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!h.state().render_pending(), "the screenshot must show the current document generation");
        let directory = std::path::Path::new(&directory);
        std::fs::create_dir_all(directory).unwrap();
        h.render().unwrap().save(directory.join(name)).unwrap();
    }
}

fn check_paragraph_overlap_drag(identical: bool, reopen_between: bool) {
    let (stationary, moving) = if identical { ("Repeated", "Repeated") } else { ("Stationary beta", "Moving alpha") };
    let content =
        format!("BT /F1 12 Tf 60 220 Td ({stationary}) Tj ET BT /F1 12 Tf 250 100 Td (Keep here) Tj ET BT /F1 12 Tf 60 320 Td ({moving}) Tj ET");
    let mut saved_base = paragraph_drag_fixture(&content, 0, 1);
    let mut h = paragraph_drag_harness(saved_base.clone(), 0);
    let before = dragged_paragraphs(&h);
    assert_eq!(before.len(), 3);
    paragraph_drag_shot(&mut h, &format!("before-{identical}-{reopen_between}.png"));
    let moving_index = before.iter().rposition(|b| b.text == moving).unwrap();
    let mut overlap = before.clone();
    for i in [1, 3] {
        overlap[moving_index].rect[i] -= 100.0;
    }
    drag_paragraph(&mut h, moving, [0.0, -100.0]);
    assert_dragged_paragraphs(&dragged_paragraphs(&h), &overlap);
    paragraph_drag_shot(&mut h, &format!("overlap-{identical}-{reopen_between}.png"));
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(4);
    assert_dragged_paragraphs(&dragged_paragraphs(&h), &before);
    assert!(h.state().session.get(h.state().views[0].id).unwrap().can_undo().is_none(), "the first release was exactly one step");
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    h.run_steps(4);
    assert_dragged_paragraphs(&dragged_paragraphs(&h), &overlap);

    if reopen_between {
        let bytes = save_paragraph_drag(&mut h, &format!("paragraph-overlap-{identical}.pdf"), &saved_base);
        saved_base = bytes.clone();
        h = paragraph_drag_harness(bytes, 0);
        assert_dragged_paragraphs(&dragged_paragraphs(&h), &overlap);
    }
    // A rewrite here used to remove the stationary box, even if its text was different.
    drag_paragraph(&mut h, moving, [150.0, 0.0]);
    let mut after = overlap.clone();
    for i in [0, 2] {
        after[moving_index].rect[i] += 150.0;
    }
    assert_dragged_paragraphs(&dragged_paragraphs(&h), &after);
    paragraph_drag_shot(&mut h, &format!("after-{identical}-{reopen_between}.png"));
    let current = paragraph_drag_content(&h);
    assert_eq!(current.matches(&format!("({stationary})")).count(), if identical { 2 } else { 1 });
    assert_eq!(current.matches(&format!("({moving})")).count(), if identical { 2 } else { 1 });
    assert_eq!(current.matches("(Keep here)").count(), 1);
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Move object"));
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(4);
    assert_dragged_paragraphs(&dragged_paragraphs(&h), &overlap);
    if !reopen_between {
        h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
        h.run_steps(4);
        assert_dragged_paragraphs(&dragged_paragraphs(&h), &before);
    }
    assert!(h.state().session.get(h.state().views[0].id).unwrap().can_undo().is_none(), "each release was exactly one step");
    if !reopen_between {
        h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
        h.run_steps(4);
        assert_dragged_paragraphs(&dragged_paragraphs(&h), &overlap);
    }
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    h.run_steps(4);
    assert_dragged_paragraphs(&dragged_paragraphs(&h), &after);
    assert!(h.state().session.get(h.state().views[0].id).unwrap().can_redo().is_none());
    let bytes = save_paragraph_drag(&mut h, &format!("paragraph-separated-{identical}-{reopen_between}.pdf"), &saved_base);
    let reopened = paragraph_drag_harness(bytes, 0);
    assert_dragged_paragraphs(&dragged_paragraphs(&reopened), &after);
    assert_eq!(paragraph_drag_content(&reopened), current);
}

#[test]
fn ordinary_paragraph_drags_preserve_overlapping_text_and_history() {
    check_paragraph_overlap_drag(false, false);
}

#[test]
fn ordinary_paragraph_drags_preserve_identical_overlapping_labels() {
    check_paragraph_overlap_drag(true, false);
}

#[test]
fn ordinary_paragraph_drags_preserve_overlap_after_save_and_reopen() {
    check_paragraph_overlap_drag(false, true);
    check_paragraph_overlap_drag(true, true);
}

#[test]
fn ordinary_paragraph_drags_preserve_partial_overlap_and_touching_labels() {
    let content = "BT /F1 12 Tf 60 220 Td (Repeated) Tj ET BT /F1 12 Tf 60 320 Td (Repeated) Tj ET";
    for fraction in [0.25, 0.75, 1.0] {
        let mut h = paragraph_drag_harness(paragraph_drag_fixture(content, 0, 1), 0);
        let mut expected = dragged_paragraphs(&h);
        assert_eq!(expected.len(), 2);
        let width = expected[1].rect[2] - expected[1].rect[0];
        // 75% overlap, 25% overlap, then exact touching of the original source rectangles.
        for offset in [[width * fraction, -100.0], [150.0, -40.0]] {
            drag_paragraph(&mut h, "Repeated", offset);
            for (i, coordinate) in expected[1].rect.iter_mut().enumerate() {
                *coordinate += offset[i % 2];
            }
            assert_dragged_paragraphs(&dragged_paragraphs(&h), &expected);
            assert_eq!(paragraph_drag_content(&h).matches("(Repeated)").count(), 2);
        }
    }
}

#[test]
fn ordinary_paragraph_drags_preserve_multiline_kerning_strings() {
    let content =
        concat!("BT /F1 12 Tf 60 220 Td (Keep here) Tj ET ", "BT /F1 12 Tf 14 TL 60 320 Td [(First) -120 ( line)] TJ T* (Second line) Tj ET");
    let mut h = paragraph_drag_harness(paragraph_drag_fixture(content, 0, 1), 0);
    let mut expected = dragged_paragraphs(&h);
    assert_eq!(expected.len(), 2);
    assert_eq!(expected[1].lines.len(), 2);
    let text = expected[1].text.clone();
    for offset in [[0.0, -100.0], [150.0, 0.0]] {
        drag_paragraph(&mut h, &text, offset);
        for (i, coordinate) in expected[1].rect.iter_mut().enumerate() {
            *coordinate += offset[i % 2];
        }
        assert_dragged_paragraphs(&dragged_paragraphs(&h), &expected);
        let current = paragraph_drag_content(&h);
        assert!(current.contains("[(First) -120 ( line)] TJ"), "original encoded strings and kerning stay byte-identical");
        for literal in ["(Keep here)", "(Second line)"] {
            assert_eq!(current.matches(literal).count(), 1);
        }
    }
}

#[test]
fn ordinary_paragraph_drag_respects_rotations_crop_and_user_unit() {
    let content = "BT /F1 12 Tf 60 220 Td (Keep here) Tj ET BT /F1 12 Tf 60 320 Td (Moving alpha) Tj ET";
    for rotation in [0, 90, 180, 270] {
        for view_rotation in [0, 90, 180, 270] {
            let mut h = paragraph_drag_harness(paragraph_drag_fixture(content, rotation, 2), view_rotation);
            let mut expected = dragged_paragraphs(&h);
            let moving = expected.iter_mut().find(|b| b.text == "Moving alpha").unwrap();
            for (i, coordinate) in moving.rect.iter_mut().enumerate() {
                *coordinate += [35.0, -27.0][i % 2];
            }
            drag_paragraph(&mut h, "Moving alpha", [35.0, -27.0]);
            assert_dragged_paragraphs(&dragged_paragraphs(&h), &expected);
        }
    }
}

#[test]
fn an_ordinary_added_text_drag_keeps_its_content_edit() {
    for panel_open in [true, false] {
        check_added_text_drag(panel_open);
    }
}

fn check_added_text_drag(panel_open: bool) {
    let content = "BT /F1 12 Tf 60 220 Td (Keep here) Tj ET";
    let mut h = paragraph_drag_harness(paragraph_drag_fixture(content, 0, 1), 0);
    let id = h.state().views[0].id;
    h.state_mut()
        .session
        .apply(
            id,
            pdfcraft_engine::Edit::AddText {
                page: 0,
                text: pdfcraft_engine::AddedText { text: "Added note".into(), rect: [100.0, 330.0, 230.0, 354.0], ..Default::default() },
            },
        )
        .unwrap();
    let original = h.state().session.save_bytes(id).unwrap();
    h = paragraph_drag_harness(original.as_ref().clone(), 0);
    h.state_mut().left_open = panel_open;
    h.run_steps(4);
    let added = |h: &Harness<'static, PdfCraftApp>| {
        let app = h.state();
        app.session.get(app.views[0].id).unwrap().added().to_vec()
    };
    let before_added = added(&h);
    assert_eq!(before_added.len(), 1);
    let before = dragged_paragraphs(&h);
    assert_eq!(before.len(), 2, "added text is also visible in the raw paragraph list");
    let mut expected = before.clone();
    let moved = expected.iter_mut().find(|b| b.text == "Added note").unwrap();
    for (i, coordinate) in moved.rect.iter_mut().enumerate() {
        *coordinate += [30.0, -20.0][i % 2];
    }
    // The same pointer hits added content and its source glyphs. Only content_ui may own it:
    // a Text target would be excluded from MoveObjects and overwrite the valid UpdateContent.
    let (from, to) = paragraph_drag_points(&h, "Added note", [30.0, -20.0]);
    drag(&mut h, from, to);
    assert!(h.state().views[0].line_editor.is_none());
    assert!(h.state().views[0].block_drag.is_none());
    assert_dragged_paragraphs(&dragged_paragraphs(&h), &expected);
    let after_added = added(&h);
    assert_eq!(after_added.len(), 1);
    let (old_rect, new_rect) = (before_added[0].content.rect(), after_added[0].content.rect());
    for i in 0..4 {
        assert!((new_rect[i] - old_rect[i] - [30.0, -20.0][i % 2]).abs() < 0.01);
    }
    assert_eq!(after_added[0].content, before_added[0].content.with_rect(new_rect));
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Edit content"));
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(4);
    assert_eq!(added(&h), before_added);
    assert_dragged_paragraphs(&dragged_paragraphs(&h), &before);
    assert!(h.state().session.get(h.state().views[0].id).unwrap().can_undo().is_none(), "one content edit, no paragraph rewrite");
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    h.run_steps(4);
    assert_eq!(added(&h), after_added);
    assert_dragged_paragraphs(&dragged_paragraphs(&h), &expected);
    let bytes = save_paragraph_drag(&mut h, &format!("paragraph-added-content-{panel_open}.pdf"), original.as_slice());
    let reopened = paragraph_drag_harness(bytes, 0);
    assert_eq!(added(&reopened), after_added);
    assert_dragged_paragraphs(&dragged_paragraphs(&reopened), &expected);
}

#[test]
fn an_ordinary_paragraph_drag_refusal_never_rewrites_the_text() {
    let content = "q BT 4 Tr /F1 12 Tf 60 320 Td (Clipping text) Tj ET Q";
    let mut h = paragraph_drag_harness(paragraph_drag_fixture(content, 0, 1), 0);
    let before = dragged_paragraphs(&h);
    let id = h.state().views[0].id;
    let bytes = h.state().session.get(id).unwrap().bytes.clone();
    let generation = h.state().session.get(id).unwrap().edit_generation();
    drag_paragraph(&mut h, "Clipping text", [30.0, -20.0]);
    let doc = h.state().session.get(id).unwrap();
    assert_eq!(doc.bytes, bytes);
    assert_eq!(doc.edit_generation(), generation);
    assert!(doc.can_undo().is_none());
    assert!(!doc.dirty);
    assert_dragged_paragraphs(&dragged_paragraphs(&h), &before);
    assert!(h.state().toast.as_ref().unwrap().0.contains("Text used as a clipping path cannot be moved with this selection."));
}

#[test]
fn an_ordinary_paragraph_drag_cannot_reuse_an_index_after_an_edit() {
    let content = "BT /F1 12 Tf 60 120 Td (Discard) Tj ET BT /F1 12 Tf 60 320 Td (Moving alpha) Tj ET BT /F1 12 Tf 60 220 Td (Keep here) Tj ET";
    let mut h = paragraph_drag_harness(paragraph_drag_fixture(content, 0, 1), 0);
    let (from, to) = paragraph_drag_points(&h, "Moving alpha", [30.0, -20.0]);
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    h.hover_at(to);
    h.run_steps(2);
    assert!(h.state().views[0].block_drag.is_some());
    let id = h.state().views[0].id;
    h.state_mut()
        .session
        .apply(id, pdfcraft_engine::Edit::EditTextBlock { page: 0, block: 0, text: String::new(), style: Default::default() })
        .unwrap();
    let after_edit = dragged_paragraphs(&h);
    assert_eq!(after_edit.len(), 2, "the surviving paragraphs get different indexes");
    assert_eq!(after_edit[1].text, "Keep here", "the old target index now names an unrelated paragraph");
    let generation = h.state().session.get(id).unwrap().edit_generation();
    h.run_steps(2);
    h.drop_at(to);
    h.run_steps(4);
    assert!(h.state().views[0].block_drag.is_none());
    assert_eq!(h.state().session.get(id).unwrap().edit_generation(), generation, "the stale release adds no edit");
    assert_dragged_paragraphs(&dragged_paragraphs(&h), &after_edit);
}

/// A quick flick: press at `from`, and the next frame already finds the pointer at `to`.
fn flick(h: &mut Harness<'static, PdfCraftApp>, from: egui::Pos2, to: egui::Pos2) {
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    h.hover_at(to);
    h.run_steps(1);
    h.drop_at(to);
    h.run_steps(4);
}

#[test]
fn a_quick_flick_on_either_edge_rewraps_the_paragraph() {
    let mut h = harness(1, |_| {});
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(2);
    let lines = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().text_lines(0)
    };
    let block = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().text_blocks(0)[0].clone()
    };
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let k = r.width() / 200.0;
    let screen = |x: f64, y: f64| egui::pos2(r.left() + x as f32 * k, r.top() + (300.0 - y as f32) * k);
    let near = |a: f64, b: f64| (a - b).abs() < 1.5;
    // Grab edges near the box's top: a rewrapped box reaches the window's bottom, where the
    // tool's toast sits over the page and takes the press.
    let grip_y = |b: &pdfcraft_engine::TextBlock| b.rect[3] - 6.0;
    // Narrow "Page 1" from its right edge to about 45 pt: it rewraps onto two lines.
    let b = block(&h);
    let edge = screen(b.rect[2], grip_y(&b)) + egui::vec2(2.0, 0.0);
    flick(&mut h, edge, edge - egui::vec2((b.rect[2] - b.rect[0] - 45.0) as f32 * k, 0.0));
    let text: Vec<String> = lines(&h).iter().map(|l| l.text.clone()).collect();
    assert_eq!(text, ["Page", "1"], "narrowed from the right edge");
    // Flick the right edge outward, past the box and its handle: back onto one line.
    let b = block(&h);
    let edge = screen(b.rect[2], grip_y(&b)) + egui::vec2(2.0, 0.0);
    flick(&mut h, edge, edge + egui::vec2(80.0 * k, 0.0));
    let text: Vec<String> = lines(&h).iter().map(|l| l.text.clone()).collect();
    assert_eq!(text, ["Page 1"], "widened by a fast drag that left the handle at once");
    // Flick the left edge inward: it rewraps and its left side follows the pointer.
    let b = block(&h);
    let edge = screen(b.rect[0], grip_y(&b)) - egui::vec2(2.0, 0.0);
    let inward = (b.rect[2] - b.rect[0] - 45.0) as f32;
    flick(&mut h, edge, edge + egui::vec2(inward * k, 0.0));
    let after = lines(&h);
    let text: Vec<String> = after.iter().map(|l| l.text.clone()).collect();
    assert_eq!(text, ["Page", "1"], "narrowed from the left edge");
    assert!(near(after[0].rect[0], b.rect[0] + inward as f64), "{:?} → {:?}", b.rect, after[0].rect);
    assert!(h.state().views[0].line_editor.is_none());
}

/// The Pages panel, in a window tall enough to show every thumbnail of a short fixture.
fn pages_panel(pages: usize) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, fixture(pages)).expect("fixture opens");
        app.set_option("panel", "pages").unwrap();
        app
    });
    h.run_steps(4);
    h
}

fn picked(h: &Harness<'static, PdfCraftApp>) -> Vec<usize> {
    h.state().views[0].selected.iter().copied().collect()
}

#[test]
fn pages_panel_command_click_picks_pages_without_moving_the_document() {
    let mut h = pages_panel(5);
    // The first ⌘-click on another page keeps the current page (1) selected too.
    h.get_by_label("Page 3").click_modifiers(Modifiers::COMMAND);
    h.run_steps(2);
    assert_eq!(picked(&h), [0, 2]);
    assert_eq!(h.state().views[0].current, 0, "picking pages does not turn the page");
    h.get_by_label_contains("2 pages selected");
    h.get_by_label("Page 5").click_modifiers(Modifiers::COMMAND);
    h.run_steps(2);
    assert_eq!(picked(&h), [0, 2, 4]);
    // ⌘-click again takes a page back out.
    h.get_by_label("Page 3").click_modifiers(Modifiers::COMMAND);
    h.run_steps(2);
    assert_eq!(picked(&h), [0, 4]);
    // Page commands act on what is picked, as in the organize grid.
    assert_eq!(h.state().views[0].target_pages(), [0, 4]);
}

#[test]
fn pages_panel_shift_click_picks_a_range_and_a_plain_click_starts_over() {
    let mut h = pages_panel(6);
    // A plain click goes to the page and is the anchor of the next range.
    h.get_by_label("Page 2").click_modifiers(Modifiers::NONE);
    h.run_steps(2);
    assert_eq!(h.state().views[0].current, 1);
    assert!(picked(&h).is_empty());
    h.get_by_label("Page 4").click_modifiers(Modifiers::SHIFT);
    h.run_steps(2);
    assert_eq!(picked(&h), [1, 2, 3]);
    assert_eq!(h.state().views[0].current, 1);
    // A second ⇧-click ranges from the same anchor, in either direction.
    h.get_by_label("Page 1").click_modifiers(Modifiers::SHIFT);
    h.run_steps(2);
    assert_eq!(picked(&h), [0, 1]);
    // ⇧ with no earlier click ranges from the current page.
    h.get_by_label("Page 6").click_modifiers(Modifiers::NONE);
    h.run_steps(2);
    assert!(picked(&h).is_empty(), "a plain click drops the selection");
    assert_eq!(h.state().views[0].current, 5);
    h.get_by_label("Page 5").click_modifiers(Modifiers::SHIFT);
    h.run_steps(2);
    assert_eq!(picked(&h), [4, 5]);
}

#[test]
fn pages_panel_escape_clears_and_deleted_pages_leave_the_selection() {
    let mut h = pages_panel(5);
    h.get_by_label("Page 2").click_modifiers(Modifiers::COMMAND);
    h.run_steps(2);
    assert_eq!(picked(&h), [0, 1]);
    // The pointer is still over the panel.
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(picked(&h).is_empty());
    // Pages that no longer exist drop out of the selection.
    h.get_by_label("Page 5").click_modifiers(Modifiers::COMMAND);
    h.run_steps(2);
    assert_eq!(picked(&h), [0, 4]);
    h.state_mut().views[0].select_pages(&[3, 4]);
    assert!(h.state_mut().execute("page.delete"));
    h.run_steps(3);
    assert_eq!(page_texts(h.state()).len(), 3);
    assert!(picked(&h).iter().all(|p| *p < 3), "{:?}", picked(&h));
}

#[test]
fn print_shortcut_offers_the_pages_picked_in_the_pages_panel() {
    let mut h = pages_panel(5);
    h.get_by_label("Page 2").click_modifiers(Modifiers::NONE);
    h.run_steps(2);
    h.get_by_label("Page 4").click_modifiers(Modifiers::COMMAND);
    h.run_steps(2);
    assert_eq!(picked(&h), [1, 3]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::P);
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(pdfcraft_ui_egui::Dialog::Print));
    assert_eq!(h.state().print_draft.which, pdfcraft_ui_egui::PrintWhich::Selected);
    assert_eq!(h.state().print_draft.selected, [1, 3]);
    h.get_by_label("Selected pages (2)");
    h.get_by_label("Sheet 1 of 2");
}

#[test]
fn dragging_pages_panel_thumbnails_reorders_pages() {
    let mut h = harness(4, |app| app.right = Some(pdfcraft_ui_egui::RightPanel::Pages));
    let before = page_texts(h.state());
    // The Pages panel is on the right; the document view may label its pages too.
    let thumb = |h: &Harness<'static, PdfCraftApp>, label: &str| {
        h.get_all_by_label(label).map(|n| n.rect()).max_by(|a, b| a.left().total_cmp(&b.left())).expect("the thumbnail")
    };
    // Grab page 3 (page 4 is below the window) and drop it above page 1.
    let (from, to) = (thumb(&h, "Page 3").center(), thumb(&h, "Page 1").center_top() + egui::vec2(0.0, 4.0));
    drag(&mut h, from, to);
    h.run_steps(4);
    let after = page_texts(h.state());
    assert_eq!(after, vec![before[2].clone(), before[0].clone(), before[1].clone(), before[3].clone()], "{after:?}");
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Move page"));
    let view = &h.state().views[0];
    assert_eq!(view.selected.iter().copied().collect::<Vec<_>>(), vec![0], "the moved page stays selected");
    assert!(view.panel_drag.is_none());
}

#[test]
fn a_thumbnail_drag_that_ends_off_the_panel_or_outlives_the_pages_is_dropped() {
    let mut h = harness(4, |app| app.right = Some(pdfcraft_ui_egui::RightPanel::Pages));
    let before = page_texts(h.state());
    // A drag left over (the panel closed mid-drag, the button released elsewhere) moves nothing.
    h.state_mut().views[0].panel_drag = Some(vec![2]);
    h.run_steps(2);
    assert!(h.state().views[0].panel_drag.is_none());
    assert_eq!(page_texts(h.state()), before);
    // A change to the document drops page indexes taken before it.
    h.state_mut().views[0].panel_drag = Some(vec![3]);
    let id = h.state().views[0].id;
    let info = h.state().session.get(id).unwrap().info.clone();
    h.state_mut().views[0].document_changed(&info);
    assert!(h.state().views[0].panel_drag.is_none());
}

/// A Pages panel thumbnail (the rightmost node with that label).
fn panel_thumb(h: &Harness<'static, PdfCraftApp>, label: &str) -> egui::Pos2 {
    h.get_all_by_label(label).map(|n| n.rect()).max_by(|a, b| a.left().total_cmp(&b.left())).expect("the thumbnail").center()
}

fn right_click_at(h: &mut Harness<'static, PdfCraftApp>, at: egui::Pos2) {
    h.hover_at(at);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: true, modifiers: Default::default() });
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
}

#[test]
fn deleting_pages_from_the_pages_panel_menu() {
    let mut h = pages_panel(4);
    let at = panel_thumb(&h, "Page 2");
    right_click_at(&mut h, at);
    h.get_by_label("Delete pages").click();
    h.run_steps(4);
    assert_eq!(page_texts(h.state()), ["Page 1", "Page 3", "Page 4"]);
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Delete page"));
    assert!(h.state().views[0].selected.is_empty(), "the deleted pages leave the selection");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert_eq!(page_texts(h.state()), ["Page 1", "Page 2", "Page 3", "Page 4"]);
}

#[test]
fn the_delete_key_deletes_pages_picked_in_the_pages_panel_but_never_every_page() {
    let mut h = pages_panel(3);
    // ⌘-click picks pages 2 and 3 (and the current page, 1: the first ⌘-click keeps it).
    h.get_by_label("Page 2").click_modifiers(Modifiers::COMMAND);
    h.run_steps(2);
    h.get_by_label("Page 3").click_modifiers(Modifiers::COMMAND);
    h.run_steps(2);
    let (p1, p3) = (panel_thumb(&h, "Page 1"), panel_thumb(&h, "Page 3"));
    h.hover_at(p3);
    h.run_steps(1);
    assert_eq!(h.state().views[0].selected.iter().copied().collect::<Vec<_>>(), vec![0, 1, 2]);
    // Every page picked: Delete does nothing, and the menu item is off.
    h.key_press(Key::Delete);
    h.run_steps(3);
    assert_eq!(page_texts(h.state()).len(), 3, "a document keeps at least one page");
    right_click_at(&mut h, p3);
    assert!(h.query_by(|n| n.label().as_deref() == Some("Delete pages") && n.is_disabled()).is_some());
    h.key_press(Key::Escape);
    h.run_steps(2);
    // Pages 2 and 3 picked (page 2 current): Delete deletes them, and page 1 becomes current.
    h.state_mut().views[0].select_pages(&[1, 2]);
    h.hover_at(p1);
    h.run_steps(1);
    h.key_press(Key::Delete);
    h.run_steps(4);
    assert_eq!(page_texts(h.state()), ["Page 1"]);
    assert_eq!(h.state().views[0].current, 0);
}

#[test]
fn host_dirty_hears_when_unsaved_work_appears_and_goes() {
    let reports = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = reports.clone();
    let mut h = harness(2, move |app| {
        app.set_option("organize", "on").unwrap();
        app.host_dirty = Some(Box::new(move |dirty| sink.lock().unwrap().push(dirty)));
    });
    assert_eq!(reports.lock().unwrap().last(), Some(&false), "a freshly opened document has nothing unsaved");
    h.get_by_label("Rotate clockwise").click();
    h.run_steps(3);
    assert_eq!(reports.lock().unwrap().last(), Some(&true), "an edit is unsaved work");

    let out = temp_path("host-dirty.pdf");
    h.state_mut().save_override = Some(out.to_string_lossy().into_owned());
    h.key_press_modifiers(Modifiers::COMMAND, Key::S);
    h.run_steps(3);
    assert_eq!(reports.lock().unwrap().last(), Some(&false), "saving clears it");
    let _ = std::fs::remove_file(out);
}

#[test]
fn one_editor_contains_mixed_styles_and_visual_rows() {
    let bytes = source_font_fixture_with_text(
        "BT 0 0.2 0.5 rg /F1 1 Tf 18 0 0 18 20 240 Tm (Heading) Tj ET \
         BT 0 g /F2 12 Tf 20 215 Td (Plain ) Tj /F1 12 Tf 0 0.2 0.5 rg (bold) Tj /F2 12 Tf 0 g ( words) Tj ET \
         BT /F2 10 Tf 20 195 Td (Last row) Tj ET",
    );
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("mixed.pdf", None, bytes).unwrap();
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(2);
    let page = h.state().views[0].page_screen_rect(0).unwrap();
    let point = egui::pos2(page.left() + 30.0 / 300.0 * page.width(), page.top() + 55.0 / 300.0 * page.height());
    h.hover_at(point);
    h.run_steps(1);
    h.drag_at(point);
    h.run_steps(1);
    h.drop_at(point);
    h.run_steps(6);
    assert_eq!(h.state().views[0].line_editor.as_ref().unwrap().text, "Heading\nPlain bold words\nLast row");
    let galley = h
        .output()
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(t) if t.galley.text().starts_with("Heading\n") => Some(&t.galley),
            _ => None,
        })
        .expect("one galley contains the whole region");
    assert_eq!(galley.rows.len(), 3);
    assert!(galley.job.sections.iter().any(|s| s.format.color == egui::Color32::BLACK));
    assert!(galley.job.sections.iter().any(|s| s.format.color == egui::Color32::from_rgb(0, 51, 128)));
    assert!(galley.job.sections.iter().any(|s| s.format.font_id.size > galley.job.sections.last().unwrap().format.font_id.size));
    let original_text = h.state().views[0].line_editor.as_ref().unwrap().text.clone();
    h.state_mut().views[0].line_editor.as_mut().unwrap().text.push_str("\nAdditional row\nAnother row");
    h.run_steps(3);
    let visible = h
        .output()
        .shapes
        .iter()
        .find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.text().ends_with("Another row") => Some(t.galley.rect.translate(t.pos.to_vec2())),
            _ => None,
        })
        .unwrap();
    let frame = h
        .output()
        .shapes
        .iter()
        .find_map(|s| match &s.shape {
            egui::Shape::Rect(r) if r.stroke.color == egui::Color32::from_rgb(0x14, 0x73, 0xE6) && r.rect.width() > 50.0 => Some(r.rect),
            _ => None,
        })
        .unwrap();
    assert!(frame.contains_rect(visible), "fallback font rows remain inside the growing frame");
    h.state_mut().views[0].line_editor.as_mut().unwrap().text = original_text;
    h.run_steps(3);
    if let Ok(path) = std::env::var("PDFCRAFT_RICH_FRAME_SHOT") {
        h.render().unwrap().save(path).unwrap();
    }
    h.state_mut().views[0].line_editor.as_mut().unwrap().text.clear();
    h.run_steps(3);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
    h.run_steps(4);
    assert!(h.state().session.get(h.state().views[0].id).unwrap().text_blocks(0).is_empty());
    assert!(dirty(&h));
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().text_blocks(0)[0].rows.len(), 3);
}

/// Opt-in private-document regression. This never captures images or saves the source file.
/// Run with PDFCRAFT_TEXT_REGRESSION_PDF=/path/to/document.pdf and --ignored.
#[test]
#[ignore = "requires an explicitly supplied local PDF"]
fn private_document_regions_open_as_one_editor() {
    let path = std::env::var("PDFCRAFT_TEXT_REGRESSION_PDF").expect("supply a local PDF path");
    let bytes = std::fs::read(&path).unwrap();
    let original = bytes.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1100.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("private.pdf", None, bytes).unwrap();
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(2);
    h.state_mut().views[0].fit = pdfcraft_ui_egui::canvas::Fit::Page;
    assert!(h.state_mut().execute("view.layout.single"));
    h.run_steps(3);
    let page_count = h.state().session.get(h.state().views[0].id).unwrap().info.pages.len();
    let mut checked = 0;
    for page_index in 0..page_count {
        h.state_mut().views[0].go_to_page(page_index);
        h.run_steps(4);
        let blocks = h.state().session.get(h.state().views[0].id).unwrap().text_blocks(page_index);
        let page = h.state().views[0].page_screen_rect(page_index).unwrap();
        let info = h.state().session.get(h.state().views[0].id).unwrap().info.pages[page_index].clone();
        let source_lines = h.state().session.get(h.state().views[0].id).unwrap().text_lines(page_index);
        let programs = h.state().session.get(h.state().views[0].id).unwrap().text_editor_fonts(page_index);
        assert!(!programs.is_empty(), "source sfnt fonts must be usable in the editor");
        let scale = page.width() / info.width;
        for (index, block) in blocks.iter().enumerate().filter(|(_, b)| b.rows.len() >= 3) {
            let line = &source_lines[block.lines[0]];
            let v = info.user_to_view((line.rect[0] + 3.0) as f32, (line.rect[1] + line.rect[3]) as f32 / 2.0);
            let point = egui::pos2(page.left() + v[0] * scale, page.top() + v[1] * scale);
            h.hover_at(point);
            h.run_steps(1);
            h.drag_at(point);
            h.run_steps(1);
            h.drop_at(point);
            h.run_steps(8);
            let editor = h.state().views[0].line_editor.as_ref().unwrap();
            assert_eq!(editor.block, index);
            assert_eq!(editor.text, block.editor_text);
            assert!(!editor.overflow, "unchanged source region must fit (page {}, region {})", page_index + 1, index + 1);
            let text_shape = h
                .output()
                .shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::Shape::Text(t) if t.galley.text() == block.editor_text => Some(t),
                    _ => None,
                })
                .expect("the whole source region is editable");
            for (section, (_, source)) in text_shape.galley.job.sections.iter().zip(block.edited_spans(&block.editor_text)) {
                if programs.iter().any(|font| font.resource == source_lines[source].font) {
                    assert!(
                        matches!(&section.format.font_id.family,
                        egui::FontFamily::Name(name) if name.starts_with("pdf-source-")),
                        "each available embedded source face is used in the editor"
                    );
                }
            }
            assert_eq!(text_shape.galley.rows.len(), block.rows.len(), "source rows stay in place");
            for (row, source_row) in text_shape.galley.rows.iter().zip(&block.rows) {
                let source = &source_lines[source_row[0]];
                let baseline = text_shape.pos.y + row.pos.y + row.glyphs[0].pos.y;
                let expected = page.top() + info.user_to_view(0.0, source.baseline()[1] as f32)[1] * scale;
                assert!((baseline - expected).abs() < 1.5, "editor baseline stays on the source baseline");
            }
            let frames = h
                .output()
                .shapes
                .iter()
                .filter(|s| {
                    matches!(&s.shape,
                egui::Shape::Rect(r) if r.stroke.color==egui::Color32::from_rgb(0x14,0x73,0xE6)
                    && r.rect.width()>50.0 && page.contains(r.rect.center()))
                })
                .count();
            assert_eq!(frames, 1, "one stable editor frame");
            h.state_mut().views[0].line_editor.as_mut().unwrap().text.push_str("\nAdditional text\nAdditional text");
            h.run_steps(3);
            let editor = h.state().views[0].line_editor.as_ref().unwrap();
            assert!(editor.overflow, "extra rows extend the block");
            let draft =
                h.state().session.get(h.state().views[0].id).unwrap().text_edit_layout(page_index, index, &editor.text, &editor.style()).unwrap();
            let bounds = draft.draft_rect.unwrap();
            assert!((bounds[3] - block.rect[3]).abs() < 0.5, "draft bounds exclude untouched neighboring regions");
            h.key_press(Key::Escape);
            h.run_steps(2);
            checked += 1;
        }
    }
    assert!(checked > 0, "the supplied PDF must contain a multiline region");
    assert!(!dirty(&h));
    assert_eq!(std::fs::read(path).unwrap(), original);
}

#[test]
fn inline_editor_preserves_source_font_and_previews_live_bounds() {
    use pdfcraft_cos::{Dict, Document, Object, Stream, write_incremental};
    use skrifa::MetadataProvider as _;
    let mut cos = Document::open(std::sync::Arc::new(source_font_fixture_with_text(
        "BT /F1 18 Tf 20 220 Td (Stable source text) Tj ET BT /F1 12 Tf 20 194 Td (The second row stays here) Tj ET",
    )))
    .unwrap();
    let bytes = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
    let face = skrifa::FontRef::new(bytes).unwrap();
    let metrics = face.metrics(skrifa::instance::Size::unscaled(), skrifa::instance::LocationRef::default());
    let glyphs = face.glyph_metrics(skrifa::instance::Size::unscaled(), skrifa::instance::LocationRef::default());
    let units = 1000.0 / f64::from(metrics.units_per_em);
    let program = cos.add(Object::Stream(Stream::flate(Dict::new(), bytes)));
    let mut descriptor = Dict::new();
    descriptor.set(b"FontFile2".to_vec(), Object::Ref(program));
    descriptor.set(b"Ascent".to_vec(), Object::Real(f64::from(metrics.ascent) * units));
    descriptor.set(b"Descent".to_vec(), Object::Real(f64::from(metrics.descent) * units));
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("TrueType"));
    font.set(b"BaseFont".to_vec(), Object::name("Inter-Regular"));
    font.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
    font.set(b"FirstChar".to_vec(), Object::Int(32));
    font.set(b"LastChar".to_vec(), Object::Int(126));
    font.set(
        b"Widths".to_vec(),
        Object::Array(
            (32..=126u8)
                .map(|c| {
                    let id = face.charmap().map(char::from(c)).unwrap();
                    Object::Real(f64::from(glyphs.advance_width(id).unwrap()) * units)
                })
                .collect(),
        ),
    );
    font.set(b"FontDescriptor".to_vec(), Object::Dict(descriptor));
    cos.update_dict(pdfcraft_cos::ObjRef::new(3, 0), |d| *d = font).unwrap();
    let bytes = write_incremental(&cos, &Default::default()).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("stable.pdf", None, bytes).unwrap();
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(2);
    let source = h.state().session.get(h.state().views[0].id).unwrap().text_blocks(0)[0].clone();
    let lines = h.state().session.get(h.state().views[0].id).unwrap().text_lines(0);
    let page = h.state().views[0].page_screen_rect(0).unwrap();
    let scale = page.width() / 300.0;
    let point = egui::pos2(page.left() + 30.0 * scale, page.top() + 75.0 * scale);
    h.hover_at(point);
    h.run_steps(1);
    h.drag_at(point);
    h.run_steps(1);
    h.drop_at(point);
    h.run_steps(8);
    assert!(!h.state().views[0].line_editor.as_ref().unwrap().overflow);
    let shape = h
        .output()
        .shapes
        .iter()
        .find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.text() == source.editor_text => Some(t),
            _ => None,
        })
        .unwrap();
    assert_eq!(shape.galley.rows.len(), 2);
    for (row, source_row) in shape.galley.rows.iter().zip(&source.rows) {
        let line = &lines[source_row[0]];
        let y = shape.pos.y + row.pos.y + row.glyphs[0].pos.y;
        assert!((y - (page.top() + (300.0 - line.baseline()[1] as f32) * scale)).abs() < 1.0);
    }
    for section in &shape.galley.job.sections {
        assert!(matches!(&section.format.font_id.family,egui::FontFamily::Name(name) if name.starts_with("pdf-source-")));
    }
    let frame = |h: &Harness<'_, PdfCraftApp>| {
        h.output()
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Rect(r) if r.stroke.color == egui::Color32::from_rgb(0x14, 0x73, 0xE6) && r.rect.width() > 100.0 => Some(r.rect),
                _ => None,
            })
            .unwrap()
    };
    let original_frame = frame(&h);
    h.state_mut().views[0].line_editor.as_mut().unwrap().text.push_str("\nExtra row that does not fit\nAnother row");
    h.run_steps(3);
    assert!(h.state().views[0].line_editor.as_ref().unwrap().overflow);
    let expanded = frame(&h);
    assert!(expanded.bottom() > original_frame.bottom() + 20.0, "the frame visibly expands around the extra rows");
    assert_eq!(expanded.left(), original_frame.left());
    assert_eq!(expanded.top(), original_frame.top());
    h.state_mut().views[0].line_editor.as_mut().unwrap().text = "Stable source text".into();
    h.run_steps(3);
    let reduced = frame(&h);
    assert!(reduced.bottom() < original_frame.bottom() - 20.0, "deleting a row visibly shrinks the block");
    h.state_mut().views[0].line_editor.as_mut().unwrap().text.push_str("\nExtra row that does not fit\nAnother row");
    h.run_steps(3);
    if let Ok(path) = std::env::var("PDFCRAFT_OVERFLOW_FRAME_SHOT") {
        h.render().unwrap().save(path).unwrap();
    }
    let id = h.state().views[0].id;
    let editor = h.state().views[0].line_editor.as_ref().unwrap();
    let draft_text = editor.text.clone();
    let measured = h.state().session.get(id).unwrap().text_edit_layout(0, 0, &draft_text, &editor.style()).unwrap().draft_rect.unwrap();
    let expected_frame = egui::Rect::from_min_max(
        egui::pos2(page.left() + measured[0] as f32 * scale, page.top() + (300.0 - measured[3] as f32) * scale),
        egui::pos2(page.left() + measured[2] as f32 * scale, page.top() + (300.0 - measured[1] as f32) * scale),
    )
    .expand(2.0);
    let live = frame(&h);
    assert!(
        (live.min - expected_frame.min).length() < 0.1 && (live.max - expected_frame.max).length() < 0.1,
        "the live frame shows the same PDF geometry as Apply/Save"
    );
    assert!(!dirty(&h), "draft measurement does not modify the document");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
    h.run_steps(3);
    assert!(dirty(&h));
    assert_eq!(h.state().session.get(id).unwrap().text_blocks(0)[0].rect, measured);
    let bytes = h.state().session.save_bytes(id).unwrap();
    let mut reopened = pdfcraft_engine::Session::new();
    let reopened_id = reopened.open("again.pdf", None, bytes, None).unwrap();
    assert_eq!(reopened.get(reopened_id).unwrap().text_blocks(0)[0].rect, measured);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().text_blocks(0)[0], source);
}

fn clipped_artwork_with_labels() -> Vec<u8> {
    clipped_artwork_with_labels_drawn(false)
}

/// `own_clip` draws the image alone in its clip (`q re W n cm Do Q`), which moves with it (#1049).
fn clipped_artwork_with_labels_drawn(own_clip: bool) -> Vec<u8> {
    use pdfcraft_cos::{Dict, Document, Object, Stream, write_incremental};
    let mut doc = Document::open(std::sync::Arc::new(fixture(2))).unwrap();
    let mut image = Dict::new();
    image.set(b"Type".to_vec(), Object::name("XObject"));
    image.set(b"Subtype".to_vec(), Object::name("Image"));
    image.set(b"Width".to_vec(), Object::Int(1));
    image.set(b"Height".to_vec(), Object::Int(1));
    image.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    image.set(b"ColorSpace".to_vec(), Object::name("DeviceRGB"));
    let image = doc.add(Object::Stream(Stream::flate(image, &[220, 235, 245])));
    for page in pdfcraft_model::pages(&doc) {
        let contents = doc.add(Object::Stream(Stream::flate(Dict::new(),
            if own_clip {
                &b"q 30 170 130 80 re W* n 800 0 0 900 -300 -400 cm /Im0 Do Q BT /F1 12 Tf 30 130 Td (Editable caption) Tj ET BT /F1 12 Tf 40 220 Td (Overlay) Tj ET BT /F1 9 Tf 154 249 Td (Corner label) Tj ET"[..]
            } else {
                &b"q 30 170 130 80 re W* n q 800 0 0 900 -300 -400 cm /Im0 Do Q Q BT /F1 12 Tf 30 130 Td (Editable caption) Tj ET BT /F1 12 Tf 40 220 Td (Overlay) Tj ET BT /F1 9 Tf 154 249 Td (Corner label) Tj ET"[..]
            })));
        let mut resources = doc.resolve(page.dict.get(b"Resources").unwrap()).as_dict().unwrap().clone();
        let mut xo = Dict::new();
        xo.set(b"Im0".to_vec(), Object::Ref(image));
        resources.set(b"XObject".to_vec(), Object::Dict(xo));
        doc.update_dict(page.obj, |d| {
            d.set(b"Resources".to_vec(), Object::Dict(resources));
            d.set(b"Contents".to_vec(), Object::Ref(contents));
        })
        .unwrap();
    }
    write_incremental(&doc, &Default::default()).unwrap()
}

#[test]
fn cropped_images_do_not_intercept_text_and_stay_inside_each_page() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1100.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("clipped.pdf", None, clipped_artwork_with_labels()).unwrap();
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("edit.edit_text"));
    h.state_mut().views[0].fit = pdfcraft_ui_egui::canvas::Fit::Page;
    assert!(h.state_mut().execute("view.layout.single"));
    h.run_steps(3);
    for page in 0..2 {
        h.state_mut().views[0].go_to_page(page);
        h.run_steps(3);
        let r = h.state().views[0].page_screen_rect(page).unwrap();
        let at = |x: f32, y: f32| egui::pos2(r.left() + x / 200.0 * r.width(), r.top() + (300.0 - y) / 300.0 * r.height());
        for (x, y, label) in [(65.0, 134.0, "Editable caption"), (65.0, 224.0, "Overlay")] {
            let p = at(x, y);
            h.hover_at(p);
            h.run_steps(1);
            h.drag_at(p);
            h.run_steps(1);
            h.drop_at(p);
            h.run_steps(3);
            assert_eq!(h.state().views[0].line_editor.as_ref().unwrap().text, label);
            assert!(h.state().views[0].image_selection.is_none(), "text wins even inside the visible artwork");
            h.key_press(Key::Escape);
            h.run_steps(2);
        }
        let p = at(95.0, 195.0);
        h.hover_at(p);
        h.run_steps(1);
        h.drag_at(p);
        h.run_steps(1);
        h.drop_at(p);
        h.run_steps(3);
        assert!(h.state().views[0].image_selection.as_ref().is_some_and(|s| s.page == page));
        for shape in &h.output().shapes {
            if let egui::Shape::Rect(rect) = &shape.shape
                && rect.stroke.color == egui::Color32::from_rgb(0x14, 0x73, 0xE6)
                && rect.rect.width() > 50.0
            {
                assert!(r.expand(3.0).contains_rect(rect.rect), "no image frame escapes its own page");
            }
        }
    }
    assert!(!dirty(&h));
}

#[test]
fn cropped_image_corner_resize_keeps_opposite_corner_and_scales_crop() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1100.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("clipped.pdf", None, clipped_artwork_with_labels_drawn(true)).unwrap();
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("edit.edit_text"));
    h.state_mut().views[0].fit = pdfcraft_ui_egui::canvas::Fit::Page;
    assert!(h.state_mut().execute("view.layout.single"));
    h.run_steps(3);
    let page = h.state().views[0].page_screen_rect(0).unwrap();
    let at = |x: f32, y: f32| egui::pos2(page.left() + x / 200.0 * page.width(), page.top() + (300.0 - y) / 300.0 * page.height());
    let p = at(95.0, 195.0);
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(3);
    let id = h.state().views[0].id;
    assert!(h.state().views[0].image_selection.is_some());
    let text = |doc: &pdfcraft_engine::Document| {
        doc.text_lines(0).into_iter().map(|l| (l.text.clone(), l.rect, l.font.clone(), l.size, l.color, l.baseline())).collect::<Vec<_>>()
    };
    let original_text = text(h.state().session.get(id).unwrap());
    // Grow then shrink using the visible top-right grip, never the off-page raw placement.
    for (factor, right, top, horizontal_only, off_centre) in [
        (1.15, 179.5, 262.0, false, false),
        (0.7, 134.65, 234.4, false, false),
        (0.75, 108.4875, 218.3, true, false),
        (1.1, 116.33625, 223.13, false, true),
    ] {
        let before = h.state().session.get(id).unwrap().page_images(0)[0].visible_rect.unwrap();
        let grip = at(before[2] as f32, before[3] as f32);
        let offset = if off_centre { egui::vec2(3.0, -3.0) } else { egui::Vec2::ZERO };
        let from = grip + offset;
        let fixed = at(before[0] as f32, before[1] as f32);
        let desired = fixed + (grip - fixed) * factor + offset;
        let to = if horizontal_only { egui::pos2(desired.x, from.y) } else { desired };
        let generation = h.state().session.get(id).unwrap().edit_generation();
        h.hover_at(from);
        h.run_steps(1);
        h.drag_at(from);
        h.run_steps(1);
        for k in 1..=5 {
            h.hover_at(from + (to - from) * (k as f32 / 5.0));
            h.run_steps(1);
        }
        assert_eq!(h.state().session.get(id).unwrap().edit_generation(), generation, "dragging previews without committing every frame");
        h.drop_at(to);
        h.run_steps(4);
        assert!(h.state().views[0].line_editor.is_none(), "a label under a selected image grip cannot take its drag");
        let after = h.state().session.get(id).unwrap().page_images(0)[0].visible_rect.unwrap();
        for (actual, expected) in after.into_iter().zip([30.0, 170.0, right, top]) {
            assert!((actual - expected).abs() < 0.05, "cropped image follows the visible grip: {actual} vs {expected}");
        }
        assert_eq!(text(h.state().session.get(id).unwrap()), original_text, "image resizing leaves captions in place");
    }
    let saved = h.state().session.save_bytes(id).unwrap();
    let mut reopened = pdfcraft_engine::Session::new();
    let re = reopened.open("again.pdf", None, saved, None).unwrap();
    assert_eq!(reopened.get(re).unwrap().page_images(0), h.state().session.get(id).unwrap().page_images(0));
}

/// While a corner is dragged, the image's pixels are drawn in the blue frame, not left at their
/// old size until release (#1671).
#[test]
fn image_pixels_follow_the_frame_while_a_corner_is_dragged() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1100.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("clipped.pdf", None, clipped_artwork_with_labels_drawn(true)).unwrap();
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("edit.edit_text"));
    h.state_mut().views[0].set_zoom(1.2);
    h.run_steps(4);
    let page = h.state().views[0].page_screen_rect(0).unwrap();
    let at = |x: f32, y: f32| egui::pos2(page.left() + x / 200.0 * page.width(), page.top() + (300.0 - y) / 300.0 * page.height());
    let p = at(95.0, 195.0);
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(3);
    assert!(h.state().views[0].image_selection.is_some());
    let frame = |h: &Harness<'static, PdfCraftApp>| {
        h.output().shapes.iter().find_map(|s| match &s.shape {
            egui::Shape::Rect(r) if r.stroke.color == egui::Color32::from_rgb(0x14, 0x73, 0xE6) && r.rect.width() > 20.0 => Some(r.rect),
            _ => None,
        })
    };
    // Textured meshes smaller than the page: the page raster itself always covers the page.
    let pictures = |h: &Harness<'static, PdfCraftApp>| -> Vec<egui::Rect> {
        h.output()
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Mesh(m) if m.texture_id != egui::TextureId::default() => Some(m.calc_bounds()),
                _ => None,
            })
            .filter(|b| b.width() < page.width() - 1.0)
            .collect()
    };
    let start = frame(&h).expect("the selected image has a frame");
    let (grip, fixed) = (start.right_top(), start.left_bottom());
    for factor in [0.6, 1.3] {
        let to = fixed + (grip - fixed) * factor;
        h.hover_at(grip);
        h.run_steps(1);
        h.drag_at(grip);
        h.run_steps(1);
        for k in 1..=4 {
            h.hover_at(grip + (to - grip) * (k as f32 / 4.0));
            h.run_steps(1);
        }
        let live = frame(&h).expect("the drag shows its frame");
        assert!((live.width() - start.width() * factor).abs() < 1.0, "the frame follows the grip: {live:?} from {start:?}");
        let drawn = pictures(&h);
        assert!(
            drawn.iter().any(|b| (b.min - live.min).length() < 0.5 && (b.max - live.max).length() < 0.5),
            "the picture is drawn in the frame {live:?}, not only at {drawn:?}"
        );
        h.drop_at(to);
        h.run_steps(4);
        h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
        h.run_steps(4);
    }
}

/// Exercise actual pointer gestures on each cropped placement, without capturing private pixels.
#[test]
#[ignore = "requires an explicitly supplied local PDF with cropped artwork"]
fn private_cropped_image_grips_resize_without_moving_the_opposite_corner() {
    let path = std::env::var("PDFCRAFT_CLIPPED_ARTWORK_PDF").expect("supply a local PDF path");
    let original = std::fs::read(&path).unwrap();
    let bytes = original.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1100.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("private.pdf", None, bytes).unwrap();
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("edit.edit_text"));
    h.state_mut().views[0].fit = pdfcraft_ui_egui::canvas::Fit::Page;
    assert!(h.state_mut().execute("view.layout.single"));
    let id = h.state().views[0].id;
    let pages = h.state().session.get(id).unwrap().info.pages.len();
    let mut checked = 0;
    for page_index in 0..pages {
        h.state_mut().views[0].go_to_page(page_index);
        h.run_steps(4);
        let doc = h.state().session.get(id).unwrap();
        let info = doc.info.pages[page_index].clone();
        let images = doc.page_images(page_index);
        let blocks = doc.text_blocks(page_index);
        let page = h.state().views[0].page_screen_rect(page_index).unwrap();
        let at = |x: f64, y: f64| {
            let v = info.user_to_view(x as f32, y as f32);
            page.min + egui::vec2(v[0], v[1]) * (page.width() / info.width)
        };
        for (index, image) in images.iter().enumerate() {
            let Some(b) = image.visible_rect else { continue };
            let blank = (1..10)
                .flat_map(|x| (1..10).map(move |y| (x, y)))
                .map(|(x, y)| (b[0] + (b[2] - b[0]) * x as f64 / 10.0, b[1] + (b[3] - b[1]) * y as f64 / 10.0))
                .find(|(x, y)| !blocks.iter().any(|l| *x >= l.rect[0] && *x <= l.rect[2] && *y >= l.rect[1] && *y <= l.rect[3]))
                .expect("image has an unlabelled selection point");
            let corners = [at(b[0], b[3]), at(b[2], b[3]), at(b[0], b[1]), at(b[2], b[1])];
            for corner in 0..4 {
                for (factor, axis) in [(0.8, 0), (1.04, 0), (0.8, 1), (0.8, 2)] {
                    let p = at(blank.0, blank.1);
                    h.hover_at(p);
                    h.run_steps(1);
                    h.drag_at(p);
                    h.run_steps(1);
                    h.drop_at(p);
                    h.run_steps(3);
                    assert_eq!(h.state().views[0].image_selection.as_ref().map(|s| s.index), Some(index));
                    let generation = h.state().session.get(id).unwrap().edit_generation();
                    let from = corners[corner];
                    let fixed = corners[3 - corner];
                    let desired = fixed + (from - fixed) * factor;
                    let to = match axis {
                        1 => egui::pos2(desired.x, from.y),
                        2 => egui::pos2(from.x, desired.y),
                        _ => desired,
                    };
                    h.hover_at(from);
                    h.run_steps(1);
                    h.drag_at(from);
                    h.run_steps(1);
                    for k in 1..=5 {
                        h.hover_at(from + (to - from) * (k as f32 / 5.0));
                        h.run_steps(1);
                        assert_eq!(h.state().session.get(id).unwrap().edit_generation(), generation, "no PDF rewrite on pointer movement");
                    }
                    h.drop_at(to);
                    h.run_steps(4);
                    let after = h.state().session.get(id).unwrap().page_images(page_index)[index].visible_rect.unwrap();
                    let actual = egui::Rect::from_two_pos(at(after[0], after[1]), at(after[2], after[3]));
                    let expected = egui::Rect::from_two_pos(fixed, desired).intersect(page);
                    assert!(
                        (actual.min - expected.min).length() < 0.2 && (actual.max - expected.max).length() < 0.2,
                        "page {}, image {}, corner {corner}: {actual:?} vs {expected:?}",
                        page_index + 1,
                        index + 1
                    );
                    assert!(h.state().views[0].line_editor.is_none());
                    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
                    h.run_steps(4);
                    checked += 1;
                }
            }
        }
    }
    assert_eq!(h.state().session.get(id).unwrap().bytes.as_slice(), original.as_slice(), "undo restored the source document in memory");
    assert_eq!(std::fs::read(path).unwrap(), original);
    println!("verified {checked} real-file corner drag gestures on {pages} pages");
}

/// Local acceptance check for PDFs containing cropped artwork. No screenshots or source saves.
#[test]
#[ignore = "requires an explicitly supplied local PDF with cropped artwork"]
fn private_document_cropped_artwork_does_not_block_text() {
    let path = std::env::var("PDFCRAFT_CLIPPED_ARTWORK_PDF").expect("supply a local PDF path");
    let original = std::fs::read(&path).unwrap();
    let bytes = original.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1100.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("private.pdf", None, bytes).unwrap();
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("edit.edit_text"));
    h.state_mut().views[0].fit = pdfcraft_ui_egui::canvas::Fit::Page;
    assert!(h.state_mut().execute("view.layout.single"));
    h.run_steps(3);
    let pages = h.state().session.get(h.state().views[0].id).unwrap().info.pages.len();
    let mut checked = 0;
    let mut cropped = 0;
    for page in 0..pages {
        h.state_mut().views[0].go_to_page(page);
        h.run_steps(4);
        let doc = h.state().session.get(h.state().views[0].id).unwrap();
        let info = doc.info.pages[page].clone();
        let blocks = doc.text_blocks(page);
        let images = doc.page_images(page);
        let r = h.state().views[0].page_screen_rect(page).unwrap();
        for image in &images {
            let visible = image.visible_rect.unwrap();
            assert!(visible[0] >= f64::from(info.crop[0]) - 0.01 && visible[1] >= f64::from(info.crop[1]) - 0.01);
            assert!(visible[2] <= f64::from(info.crop[2]) + 0.01 && visible[3] <= f64::from(info.crop[3]) + 0.01);
            if image.rect != visible {
                cropped += 1;
            }
        }
        for (index, block) in blocks.iter().enumerate() {
            let [x0, y0, x1, y1] = block.rect;
            let v = info.user_to_view(((x0 + x1) / 2.0) as f32, ((y0 + y1) / 2.0) as f32);
            let scale = r.width() / info.width;
            let p = egui::pos2(r.left() + v[0] * scale, r.top() + v[1] * scale);
            h.hover_at(p);
            h.run_steps(1);
            h.drag_at(p);
            h.run_steps(1);
            h.drop_at(p);
            h.run_steps(3);
            assert_eq!(h.state().views[0].line_editor.as_ref().map(|e| e.block), Some(index), "every source block can open directly");
            assert!(h.state().views[0].image_selection.is_none());
            h.key_press(Key::Escape);
            h.run_steps(2);
            checked += 1;
        }
    }
    assert!(checked > 0 && cropped > 0);
    assert!(!dirty(&h));
    assert_eq!(std::fs::read(path).unwrap(), original);
    println!("checked {pages} pages, {checked} text regions, {cropped} clipped image placements");
}

/// Actual keyboard input into every page of a private Chinese PDF; no private captures.
#[test]
#[ignore = "requires a private Chinese PDF and an approved full source font file"]
fn private_new_chinese_keyboard_input_preserves_font_layout_and_saved_pixels() {
    use pdfcraft_engine::{Edit, Session};
    use std::sync::Arc;
    let path = std::env::var("PDFCRAFT_CHINESE_TEXT_PDF").expect("supply a local PDF path");
    let font_path = std::env::var("PDFCRAFT_CHINESE_FULL_FONT").expect("supply the full source face");
    let original = std::fs::read(&path).unwrap();
    let bytes = original.clone();
    let font = Arc::new(std::fs::read(font_path).unwrap());
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1100.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("private.pdf", None, bytes).unwrap();
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("edit.edit_text"));
    h.state_mut().views[0].fit = pdfcraft_ui_egui::canvas::Fit::Page;
    assert!(h.state_mut().execute("view.layout.single"));
    let id = h.state().views[0].id;
    let pages = h.state().session.get(id).unwrap().info.pages.len();
    let all: String = (0..pages).flat_map(|p| h.state().session.get(id).unwrap().text_lines(p)).map(|l| l.text).collect();
    let new: String = "测试编辑删除龘".chars().filter(|c| !all.contains(*c)).take(4).collect();
    assert!(new.chars().count() >= 2);
    let pixels = |bytes: Arc<Vec<u8>>, page| {
        let mut r = PageRenderer::new(bytes, Default::default());
        let p = r.render(RenderRequest { page, scale: 1.5, ..Default::default() });
        assert!(p.error.is_none() && p.warnings.is_empty());
        p.rgba.to_vec()
    };
    for page_index in 0..pages {
        h.state_mut().views[0].go_to_page(page_index);
        h.state_mut().session.apply(id, Edit::RegisterTextFont { page: page_index, bytes: font.clone() }).unwrap();
        h.run_steps(4);
        let doc = h.state().session.get(id).unwrap();
        let (index, block) = doc
            .text_blocks(page_index)
            .into_iter()
            .enumerate()
            .find(|(_, b)| b.rows.len() == 1 && b.editor_text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)))
            .unwrap();
        let source = doc.text_lines(page_index)[block.lines[0]].clone();
        let info = doc.info.pages[page_index].clone();
        let page = h.state().views[0].page_screen_rect(page_index).unwrap();
        let scale = page.width() / info.width;
        let v = info.user_to_view((source.rect[0] + 3.0) as f32, ((source.rect[1] + source.rect[3]) / 2.0) as f32);
        let point = page.min + egui::vec2(v[0], v[1]) * scale;
        h.hover_at(point);
        h.run_steps(1);
        h.drag_at(point);
        h.run_steps(1);
        h.drop_at(point);
        h.run_steps(8);
        assert_eq!(h.state().views[0].line_editor.as_ref().unwrap().block, index);
        let draft = format!("{}{new}", block.editor_text);
        h.key_press_modifiers(Modifiers::COMMAND, Key::A);
        h.event(egui::Event::Text(draft.clone()));
        h.run_steps(4);
        assert_eq!(h.state().views[0].line_editor.as_ref().unwrap().text, draft);
        let galley = h
            .output()
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == draft => Some(t.galley.clone()),
                _ => None,
            })
            .expect("typed Chinese is in the actual inline editor");
        assert!(
            galley
                .job
                .sections
                .iter()
                .filter(|s| draft
                    .get(usize::from(s.byte_range.start)..usize::from(s.byte_range.end))
                    .is_some_and(|text| text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))))
                .all(|s| matches!(&s.format.font_id.family, egui::FontFamily::Name(n) if n.starts_with("pdf-source-")))
        );
        h.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
        h.run_steps(5);
        assert!(h.state().views[0].line_editor.is_none());
        let edited = h.state().session.get(id).unwrap();
        assert!(edited.text_lines(page_index).iter().any(|l| l.text.contains(&new) && l.base_font == "DroidSansFallback"));
        let first = edited.text_lines(page_index).into_iter().find(|l| l.text.starts_with(source.text.trim())).unwrap();
        assert!(first.baseline().iter().zip(source.baseline()).all(|(a, b)| (a - b).abs() < 0.01));
        let current = pixels(edited.display.clone(), page_index);
        let saved = h.state_mut().session.save_bytes(id).unwrap();
        let mut reopen = Session::new();
        let rid = reopen.open("saved.pdf", None, saved, None).unwrap();
        assert_eq!(pixels(reopen.get(rid).unwrap().display.clone(), page_index), current);
        // Repeat keyboard input and deletion without recreating the editor via state mutation.
        h.hover_at(point);
        h.run_steps(1);
        h.drag_at(point);
        h.run_steps(1);
        h.drop_at(point);
        h.run_steps(6);
        assert!(h.state().views[0].line_editor.as_ref().unwrap().text.contains(&new));
        h.key_press_modifiers(Modifiers::COMMAND, Key::A);
        let shortened = draft.replace(&new, &new.chars().skip(1).collect::<String>());
        h.event(egui::Event::Text(shortened.clone()));
        h.run_steps(3);
        h.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
        h.run_steps(4);
        let current = pixels(h.state().session.get(id).unwrap().display.clone(), page_index);
        let saved = h.state_mut().session.save_bytes(id).unwrap();
        let rid = reopen.open("deleted.pdf", None, saved, None).unwrap();
        assert_eq!(pixels(reopen.get(rid).unwrap().display.clone(), page_index), current);
        let content = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
        assert_eq!(content(&reopen.get(rid).unwrap().text_blocks(page_index)[index].editor_text), content(&shortened));
        h.state_mut().session.undo(id).unwrap();
        h.state_mut().session.undo(id).unwrap();
        h.state_mut().session.undo(id).unwrap();
        h.run_steps(4);
    }
    assert_eq!(std::fs::read(path).unwrap(), original);
    println!("verified Chinese keyboard insertion/deletion and save/reopen rendering on {pages} pages");
}

/// Moving text shows the four-way arrow wherever a drag would move it (#1520 follow-up): over a
/// closed paragraph, on the frame of an open one, and while dragging. Inside the open one the
/// pointer reads as text; on a closed one's edge, as the rewrap handle.
#[test]
fn moving_text_offers_the_four_way_arrow() {
    let mut h = harness(2, |_| {});
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(2);
    let id = h.state().views[0].id;
    let block = h.state().session.get(id).unwrap().text_blocks(0)[0].clone();
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let k = r.width() / 200.0;
    let screen = |x: f64, y: f64| egui::pos2(r.left() + x as f32 * k, r.top() + (300.0 - y as f32) * k);
    let centre = screen((block.rect[0] + block.rect[2]) / 2.0, (block.rect[1] + block.rect[3]) / 2.0);
    let cursor_at = |h: &mut Harness<'static, PdfCraftApp>, at: egui::Pos2| {
        h.hover_at(at);
        h.run_steps(2);
        h.output().platform_output.cursor_icon
    };
    // A closed paragraph: a drag moves it; its left edge rewraps it.
    assert_eq!(cursor_at(&mut h, centre), egui::CursorIcon::Move);
    assert_eq!(cursor_at(&mut h, screen(block.rect[0], (block.rect[1] + block.rect[3]) / 2.0)), egui::CursorIcon::ResizeHorizontal);
    // While it is being dragged.
    h.hover_at(centre);
    h.run_steps(1);
    h.drag_at(centre);
    h.run_steps(1);
    let mid = centre + egui::vec2(20.0 * k, 10.0 * k);
    h.hover_at(mid);
    h.run_steps(2);
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::Move);
    h.drop_at(mid);
    h.run_steps(4);
    let moved = h.state().session.get(id).unwrap().text_blocks(0)[0].clone();
    assert!((moved.rect[0] - block.rect[0] - 20.0).abs() < 1.5, "{:?} → {:?}", block.rect, moved.rect);
    // Open: text inside, the frame moves it.
    let centre = screen((moved.rect[0] + moved.rect[2]) / 2.0, (moved.rect[1] + moved.rect[3]) / 2.0);
    click_at(&mut h, centre);
    assert!(h.state().views[0].line_editor.is_some());
    assert_eq!(cursor_at(&mut h, centre), egui::CursorIcon::Text);
    let frame = screen((moved.rect[0] + moved.rect[2]) / 2.0, moved.rect[3]) - egui::vec2(0.0, 4.0);
    assert_eq!(cursor_at(&mut h, frame), egui::CursorIcon::Move);
}

/// Cropping a picture on the page (Acrobat's crop handles): in crop mode the
/// handles trim, Enter applies what is kept, Esc leaves the picture as it was.
#[test]
fn cropping_a_picture_with_its_handles() {
    let mut png = Vec::new();
    image::RgbImage::from_pixel(80, 40, image::Rgb([200, 40, 40])).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("picture.png", None, png.clone()).expect("opens");
        app
    });
    h.run_steps(4);
    let id = h.state().views[0].id;
    let before = h.state().session.get(id).unwrap().page_images(0)[0].visible_rect.unwrap();
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(2);
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let c = r.center();
    h.hover_at(c);
    h.run_steps(1);
    h.drag_at(c);
    h.run_steps(1);
    h.drop_at(c);
    h.run_steps(2);
    assert!(h.state().views[0].image_selection.is_some(), "selected");
    // Crop mode (as the context menu's Crop Image starts it): drag the right-bottom handle a
    // quarter of the picture inward, then Enter.
    h.state_mut().views[0].image_selection.as_mut().unwrap().crop = Some(pdfcraft_ui_egui::CropMode::new(before));
    h.run_steps(2);
    let corner = r.right_bottom();
    let to = corner - egui::vec2(r.width() / 4.0, r.height() / 4.0);
    h.hover_at(corner);
    h.run_steps(1);
    h.drag_at(corner);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(corner + (to - corner) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    h.drop_at(to);
    h.run_steps(2);
    let keep = h.state().views[0].image_selection.as_ref().unwrap().crop.as_ref().unwrap().keep;
    assert!(keep[2] < before[2] - 1.0 && keep[1] > before[1] + 1.0, "trimmed on the right and the bottom: {keep:?} of {before:?}");
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    let doc = h.state().session.get(id).unwrap();
    assert_eq!(doc.can_undo(), Some("Crop image"));
    let im = &doc.page_images(0)[0];
    assert_eq!(im.rect, [0.0, 0.0, 80.0, 40.0], "the placement is not scaled");
    let v = im.visible_rect.unwrap();
    assert!((v[2] - 60.0).abs() < 1.5 && (v[1] - 10.0).abs() < 1.5, "kept {v:?}");
    assert!(h.state().views[0].image_selection.as_ref().is_some_and(|s| s.crop.is_none()), "crop mode ends");
    // Esc leaves it.
    h.state_mut().views[0].image_selection.as_mut().unwrap().crop = Some(pdfcraft_ui_egui::CropMode::new([10.0, 10.0, 20.0, 20.0]));
    h.run_steps(2);
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    let doc = h.state().session.get(id).unwrap();
    assert_eq!(doc.page_images(0)[0].visible_rect.unwrap(), v);
    assert!(h.state().views[0].image_selection.as_ref().is_none_or(|s| s.crop.is_none()));
}

/// Organize pages has Acrobat's panel whenever the grid shows, however it was opened, and the
/// panel's Close ends Organize: the grid closes and All tools comes back.
#[test]
fn organize_panel_comes_with_the_grid_and_closes_it() {
    let mut h = organize(3);
    h.get_by_label("Insert");
    h.get_by_label("Bates numbering");
    h.get_by_label("Page properties");
    h.get_by_label("Close panel").click();
    h.run_steps(3);
    assert!(!h.state().views[0].organize, "Close ends Organize");
    assert_eq!(h.state().left, pdfcraft_ui_egui::LeftPanel::AllTools);
    assert!(h.query_by_label("Page properties").is_none(), "the panel is gone");
}

/// Leaving Organize (a page opened from the grid, or the grid's Close) goes back to the
/// workspace's panel: under the Edit tab, the Edit panel, not All tools.
#[test]
fn leaving_organize_goes_back_to_the_workspace_panel() {
    use pdfcraft_ui_egui::{LeftPanel, Mode};
    let mut h = harness(3, |_| {});
    h.state_mut().mode = Mode::Edit;
    h.state_mut().left = LeftPanel::Tool("edit");
    h.run_steps(2);
    h.state_mut().left = LeftPanel::Tool("organize");
    h.state_mut().views[0].organize = true;
    h.run_steps(3);
    h.get_by_label("Pages selected".to_uppercase().as_str());
    // What opening a page from the grid does.
    h.state_mut().views[0].organize = false;
    h.run_steps(3);
    assert_eq!(h.state().left, LeftPanel::Tool("edit"), "back to the Edit panel");
    // The same from All tools goes back to All tools.
    h.state_mut().mode = Mode::AllTools;
    h.state_mut().left = LeftPanel::Tool("organize");
    h.state_mut().views[0].organize = true;
    h.run_steps(3);
    h.state_mut().views[0].organize = false;
    h.run_steps(3);
    assert_eq!(h.state().left, LeftPanel::AllTools);
}
