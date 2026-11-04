//! Focused logical text, caret and composition journeys through the real PdfCraft shell.

use std::sync::{Arc, Mutex};

use egui::{Event, ImeEvent, Key, Modifiers};
use egui_kittest::{Harness, kittest::Queryable};
use pdfcraft_ui_egui::{PdfCraftApp, control::ControlClient};
use serde_json::{Value, json};

/// Create the default synthetic page used to open either page-text editor.
fn fixture() -> Vec<u8> {
    let body = "BT /F1 24 Tf 20 150 Td (Page 1) Tj ET";
    fixture_with_body(body)
}

/// Build a single-page synthetic PDF using the supplied text operators as its content stream.
fn fixture_with_body(body: &str) -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 4 0 R >> >> >>".to_owned(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    bytes
}

/// Open the default fixture in the selected existing-text or added-text editing path.
fn harness(source: bool) -> (Harness<'static, PdfCraftApp>, ControlClient) {
    harness_with_pdf(source, fixture(), false)
}

/// Open the supplied synthetic PDF and focus either the existing-text or added-text editor with opt-in control attached.
fn harness_with_pdf(source: bool, bytes: Vec<u8>, fit_page: bool) -> (Harness<'static, PdfCraftApp>, ControlClient) {
    let slot = Arc::new(Mutex::new(None));
    let capture = Arc::clone(&slot);
    let mut builder = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(0.25);
    if std::env::var_os("PDFCRAFT_INPUT_SNAPSHOT").is_some() {
        builder = builder.wgpu();
    }
    let mut h = builder.build_eframe(move |cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        if fit_page {
            app.set_option("default-zoom", "fit-page").unwrap();
        }
        *capture.lock().unwrap() = Some(app.attach_control(&cc.egui_ctx));
        app.open_bytes("input.pdf", None, bytes).unwrap();
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute(if source { "edit.edit_text" } else { "edit.text" }));
    h.run_steps(3);
    let page = h.state().views[0].page_screen_rect(0).unwrap();
    let at = if source {
        egui::pos2(page.left() + page.width() * 0.2, page.top() + page.height() * (142.0 / 300.0))
    } else {
        egui::pos2(page.left() + page.width() * 0.1, page.top() + page.height() * (40.0 / 300.0))
    };
    click(&mut h, at);
    assert!(if source { h.state().views[0].line_editor.is_some() } else { h.state().views[0].content.draft.is_some() });
    let client = slot.lock().unwrap().take().unwrap();
    (h, client)
}

/// Deliver a primary pointer press/release and let the native UI settle.
fn click(h: &mut Harness<'static, PdfCraftApp>, at: egui::Pos2) {
    h.hover_at(at);
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    }
    h.run_steps(3);
}

/// Pass synthetic input through the real app raw-input hook before advancing the native harness.
fn native_input(h: &mut Harness<'static, PdfCraftApp>, events: Vec<Event>) {
    // Kittest's eframe adapter starts at App::logic, skipping raw_input_hook.
    // Exercise the real host hook before egui can move focus for Escape/Tab.
    // Put a physical key and its IME update in the same native input frame.
    let mut raw = std::mem::take(h.input_mut());
    raw.events.extend(events);
    let ctx = h.ctx.clone();
    eframe::App::raw_input_hook(h.state_mut(), &ctx, &mut raw);
    *h.input_mut() = raw;
    h.run_steps(3);
}

/// Construct matching native key-down/key-up events without modifiers.
fn key_events(key: Key) -> Vec<Event> {
    [true, false].into_iter().map(|pressed| Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE }).collect()
}

/// Send a UI-control request and advance a bounded number of frames until it receives a reply.
fn call(h: &mut Harness<'static, PdfCraftApp>, c: &ControlClient, method: &str, args: Value) -> Result<Value, String> {
    let rx = c.send(method, args);
    for _ in 0..120 {
        h.run_steps(1);
        if let Ok(reply) = rx.try_recv() {
            return reply;
        }
    }
    panic!("{method} did not answer");
}

/// Read the current logical draft from the selected existing-text or added-text editor.
fn text(h: &Harness<'static, PdfCraftApp>, source: bool) -> String {
    let view = &h.state().views[0];
    if source {
        view.line_editor.as_ref().expect("source editor stays open").text.clone()
    } else {
        view.content.draft.as_ref().expect("added editor stays open").text.clone()
    }
}

/// Set the native TextEdit selection using Unicode character positions.
fn select(h: &mut Harness<'static, PdfCraftApp>, start: usize, end: usize) {
    let id = h.ctx.memory(|m| m.focused()).expect("text has keyboard focus");
    let mut state = egui::TextEdit::load_state(&h.ctx, id).unwrap();
    state.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(start), egui::text::CCursor::new(end))));
    state.store(&h.ctx, id);
}

/// Replace the whole focused draft through native text input and require the expected logical value.
fn replace(h: &mut Harness<'static, PdfCraftApp>, source: bool, value: &str) {
    select(h, 0, text(h, source).chars().count());
    h.event(Event::Text(value.into()));
    h.run_steps(3);
    assert_eq!(text(h, source), value);
}

/// Read the primary native caret position in Unicode characters.
fn caret(h: &Harness<'static, PdfCraftApp>) -> usize {
    let id = h.ctx.memory(|m| m.focused()).unwrap();
    egui::TextEdit::load_state(&h.ctx, id).unwrap().cursor.char_range().unwrap().primary.index.0
}

#[test]
/// Verify invalid composition payloads leave both editor drafts and keyboard focus unchanged.
fn malformed_composition_requests_preserve_text_and_focus() {
    for source in [true, false] {
        let (mut h, c) = harness(source);
        let before = text(&h, source);
        let owner = h.ctx.memory(|m| m.focused());
        for args in [
            json!({"phase":"preedit","text":"مَر","selection":[0,4]}),
            json!({"phase":"preedit","text":"مَر","selection":[2,1]}),
            json!({"phase":"preedit","text":"مَر","selection":[-1,1]}),
            json!({"phase":"commit","text":"ع","selection":[0,1]}),
            json!({"phase":"cancel","text":"unexpected"}),
            json!({"phase":"unknown","text":"ع"}),
            json!({"phase":"commit","text":"ع".repeat(131073)}),
        ] {
            assert!(call(&mut h, &c, "ui.ime", args).is_err());
            assert_eq!(text(&h, source), before);
            assert_eq!(h.ctx.memory(|m| m.focused()), owner);
        }
        select(&mut h, 0, before.chars().count());
        call(&mut h, &c, "ui.ime", json!({"phase":"preedit","text":""})).unwrap();
        assert_eq!(text(&h, source), before, "an idle empty preedit must not erase selection");
    }
}

#[test]
/// Verify native composition cancellation retains the accepted draft and keeps either page editor open.
fn composition_cancel_keeps_both_page_editors_open() {
    for source in [true, false] {
        let (mut h, c) = harness(source);
        let before = text(&h, source);
        let end = before.chars().count();
        select(&mut h, end, end);
        call(&mut h, &c, "ui.ime", json!({"phase":"preedit","text":"مر","selection":[0,2]})).unwrap();
        assert_eq!(text(&h, source), format!("{before}مر"));
        // The same physical Escape cancels composition; it must not discard the PDF draft.
        let mut events = vec![Event::Ime(ImeEvent::Preedit { text: String::new(), active_range_chars: None })];
        events.extend(key_events(Key::Escape));
        native_input(&mut h, events);
        assert_eq!(text(&h, source), before);
        // A subsequent Escape is an ordinary editor command again.
        h.key_press(Key::Escape);
        h.run_steps(3);
        assert!(if source { h.state().views[0].line_editor.is_none() } else { h.state().views[0].content.draft.is_none() });
    }
}

#[test]
/// Verify confirmation keys and Save cannot publish an unfinished preedit into either page-text path.
fn composition_enter_tab_and_save_cannot_publish_a_candidate() {
    for source in [true, false] {
        let (mut h, c) = harness(source);
        replace(&mut h, source, "ABC");
        select(&mut h, 3, 3);
        call(&mut h, &c, "ui.ime", json!({"phase":"preedit","text":"مرح"})).unwrap();
        let owner = h.ctx.memory(|m| m.focused());
        for key in [Key::Enter, Key::Tab] {
            native_input(&mut h, key_events(key));
            assert_eq!(h.ctx.memory(|m| m.focused()), owner);
            assert_eq!(text(&h, source), "ABCمرح");
        }
        h.state_mut().execute("file.save");
        h.run_steps(3);
        assert_eq!(text(&h, source), "ABCمرح");
        assert!(h.state().toast.as_ref().is_some_and(|(message, _)| message.contains("Finish composing")));
        call(&mut h, &c, "ui.ime", json!({"phase":"commit","text":"مرحبا"})).unwrap();
        assert_eq!(text(&h, source), "ABCمرحبا");
    }
}

/// The focused form widget must receive a live preedit, and Save must leave both
/// its accepted value and disk bytes unchanged until that same widget commits.
/// ASCII isolates composition ownership from optional script-font coverage.
#[test]
fn form_composition_save_preserves_preedit_until_commit_and_persists_commit() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-form-ime-save-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("saved.pdf");
    let bytes = include_bytes!("data/form.pdf").to_vec();
    std::fs::write(&out, &bytes).unwrap();
    let path = out.to_string_lossy().into_owned();
    let slot = Arc::new(Mutex::new(None));
    let capture = Arc::clone(&slot);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(0.25).build_eframe(move |cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        *capture.lock().unwrap() = Some(app.attach_control(&cc.egui_ctx));
        app.open_bytes("form-ime.pdf", Some(path), bytes).unwrap();
        app.set_option("left", "closed").unwrap();
        app.set_option("zoom", "150").unwrap();
        app
    });
    h.run_steps(4);
    let client = slot.lock().unwrap().take().unwrap();
    let field_value = |h: &Harness<'static, PdfCraftApp>| {
        h.state().session.get(h.state().views[0].id).unwrap().form.iter().find(|field| field.name == "name").unwrap().value.clone()
    };
    let before_value = field_value(&h);
    let at = {
        let app = h.state();
        let doc = app.session.get(app.views[0].id).unwrap();
        let field = doc.form.iter().find(|field| field.name == "name").unwrap();
        pdfcraft_ui_egui::forms_ui::field_screen_rect(&app.views[0], &doc.info, field, 0).expect("form field is visible").center()
    };
    click(&mut h, at);
    let focus = h.state().views[0].forms.focus.as_ref().expect("actual form editor is open");
    assert_eq!(focus.name, "name");
    let before_draft = focus.text.clone();
    select(&mut h, 0, before_draft.chars().count());
    let owner = h.ctx.memory(|memory| memory.focused()).expect("form editor has native keyboard focus");
    call(&mut h, &client, "ui.ime", json!({"phase":"preedit","text":"candidate"})).unwrap();
    let focus = h.state().views[0].forms.focus.as_ref().unwrap();
    assert_eq!((focus.name.as_str(), focus.text.as_str()), ("name", "candidate"), "ui.ime must reach the focused form widget");
    assert_eq!(h.ctx.memory(|memory| memory.focused()), Some(owner));
    let ime = h.output().platform_output.ime.as_ref().expect("focused form emits native IME state");
    let egui::output::IMESurroundingText::Available(payload) = &ime.surrounding_text else { panic!("short form committed-text snapshot") };
    assert_eq!(payload.text, before_draft, "native committed text must exclude the live candidate");
    let before_saved = std::fs::read(&out).unwrap();
    assert!(h.state_mut().execute("file.save"));
    h.run_steps(3);
    assert!(h.state().toast.as_ref().is_some_and(|(message, _)| message.contains("Finish composing text before applying this action.")));
    assert_eq!(field_value(&h), before_value, "Save cannot commit preedit into the document field");
    assert_eq!(std::fs::read(&out).unwrap(), before_saved, "blocked Save cannot replace the existing file");
    assert_eq!(h.state().views[0].forms.focus.as_ref().unwrap().text, "candidate");
    call(&mut h, &client, "ui.ime", json!({"phase":"commit","text":"Committed value"})).unwrap();
    assert_eq!(h.state().views[0].forms.focus.as_ref().unwrap().text, "Committed value");
    assert!(h.state_mut().execute("file.save"));
    h.run_steps(3);
    assert_eq!(field_value(&h), ["Committed value"]);
    let mut reopened = PdfCraftApp::new();
    reopened.open_bytes("saved.pdf", None, std::fs::read(&out).unwrap()).unwrap();
    let saved_field = reopened.session.get(reopened.views[0].id).unwrap().form.iter().find(|field| field.name == "name").unwrap();
    assert_eq!(saved_field.value, ["Committed value"], "the committed form value must survive the actual saved PDF");
    std::fs::remove_file(out).unwrap();
    std::fs::remove_dir(dir).unwrap();
}

#[test]
/// Verify an outside paragraph click cannot publish preedit, while cancellation restores ordinary switching.
fn composition_blocks_switching_to_another_paragraph() {
    let bytes = fixture_with_body("BT /F1 24 Tf 20 150 Td (Page 1) Tj ET BT /F1 24 Tf 20 70 Td (Second) Tj ET");
    let other_point = |h: &Harness<'static, PdfCraftApp>| {
        let doc = &h.state().session.docs()[0];
        let rect = doc.text_blocks(0)[1].rect;
        let [x, y] = doc.info.pages[0].user_to_view(((rect[0] + rect[2]) / 2.0) as f32, ((rect[1] + rect[3]) / 2.0) as f32);
        let page = h.state().views[0].page_screen_rect(0).unwrap();
        let point = egui::pos2(page.left() + page.width() * x / 200.0, page.top() + page.height() * y / 300.0);
        assert!(h.state().views[0].viewport_rect().contains(point), "the other paragraph must be visible");
        point
    };
    let host_click = |h: &mut Harness<'static, PdfCraftApp>, at| {
        h.hover_at(at);
        h.run_steps(1);
        let events = [true, false]
            .into_iter()
            .map(|pressed| Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE })
            .collect();
        // Like real eframe hosts, run raw_input_hook before egui processes focus.
        native_input(h, events);
    };
    // Positive control proves the real router can switch to this visible paragraph.
    let (mut ordinary, _) = harness_with_pdf(true, bytes.clone(), true);
    replace(&mut ordinary, true, "ABC");
    let at = other_point(&ordinary);
    host_click(&mut ordinary, at);
    assert_eq!(ordinary.state().views[0].line_editor.as_ref().unwrap().block, 1, "inactive composition must permit switching");
    let (mut h, c) = harness_with_pdf(true, bytes, true);
    replace(&mut h, true, "ABC");
    select(&mut h, 3, 3);
    // ASCII preedit isolates composition ownership from font fallback behavior.
    call(&mut h, &c, "ui.ime", json!({"phase":"preedit","text":"candidate"})).unwrap();
    let before = h.state().session.docs()[0].text_blocks(0);
    let other = other_point(&h);
    host_click(&mut h, other);
    assert_eq!(h.state().session.docs()[0].text_blocks(0), before, "switching paragraphs must not publish a live preedit");
    assert_eq!(text(&h, true), "ABCcandidate", "the composition draft stays open");
    assert_eq!(h.state().views[0].line_editor.as_ref().unwrap().block, 0);
    call(&mut h, &c, "ui.ime", json!({"phase":"cancel"})).unwrap();
    host_click(&mut h, other);
    assert_eq!(h.state().views[0].line_editor.as_ref().unwrap().block, 1, "switching resumes after composition ends");
    assert_eq!(h.state().session.docs()[0].text_blocks(0)[0].editor_text, "ABC");
}

#[test]
/// Check native visual caret movement and logical selection/copy across mixed Arabic, Latin and digits.
fn mixed_arabic_caret_selection_and_copy_keep_logical_order() {
    for source in [true, false] {
        let (mut h, _) = harness(source);
        replace(&mut h, source, "مرحبا ABC 123");
        select(&mut h, 2, 2);
        h.key_press(Key::ArrowLeft);
        h.run_steps(2);
        assert_eq!(caret(&h), 3, "left moves visually through the RTL run");
        h.key_press(Key::ArrowRight);
        h.run_steps(2);
        assert_eq!(caret(&h), 2);
        h.key_press_modifiers(Modifiers::SHIFT, Key::ArrowLeft);
        h.run_steps(2);
        h.event(Event::Text("س".into()));
        h.run_steps(2);
        assert_eq!(text(&h, source), "مرسبا ABC 123");
        let count = text(&h, source).chars().count();
        select(&mut h, 0, count);
        h.event(Event::Copy);
        h.run_steps(1);
        assert!(h.output().platform_output.commands.iter().any(|c| matches!(c, egui::OutputCommand::CopyText(value) if value == "مرسبا ABC 123")));
    }
}

#[test]
/// Verify native deletion edits Arabic marks and multiline text without inventing a separate cursor model.
fn marks_and_multiline_deletion_use_native_grapheme_boundaries() {
    for source in [true, false] {
        let (mut h, _) = harness(source);
        replace(&mut h, source, "بَت");
        select(&mut h, 2, 2);
        h.key_press(Key::Backspace);
        h.run_steps(2);
        assert_eq!(text(&h, source), "بت", "Backspace corrects the haraka without removing its letter");
        h.key_press(Key::Backspace);
        h.run_steps(2);
        assert_eq!(text(&h, source), "ت");
        replace(&mut h, source, "بَت");
        select(&mut h, 0, 0);
        h.key_press(Key::Delete);
        h.run_steps(2);
        assert_eq!(text(&h, source), "ت");
        replace(&mut h, source, "مرحبا\nABC 123");
        select(&mut h, 6, 6);
        h.key_press(Key::Backspace);
        h.run_steps(2);
        assert_eq!(text(&h, source), "مرحباABC 123", "joining lines preserves typing order");
        h.key_press(Key::Enter);
        h.run_steps(2);
        assert_eq!(text(&h, source), "مرحبا\nABC 123");
    }
}

#[test]
/// Verify pointer placement within rendered Arabic inserts text at the native visible caret in both page editors.
fn clicking_inside_arabic_text_inserts_at_the_visible_caret() {
    if !arabic_font_available() {
        return;
    }
    for source in [true, false] {
        let (mut h, _) = harness(source);
        replace(&mut h, source, "مرحبا");
        let bounds = h.get_by_label(if source { "Text editing line" } else { "Added text" }).rect();
        click(&mut h, egui::pos2(bounds.left() + 2.0, bounds.center().y));
        assert_eq!(caret(&h), 5, "left edge is the logical end of the RTL word");
        click(&mut h, egui::pos2(bounds.right() - 2.0, bounds.center().y));
        assert_eq!(caret(&h), 0, "right edge is the logical beginning");
        let page_scale = h.state().views[0].page_screen_rect(0).unwrap().width() / 200.0;
        let size = if source {
            h.state().views[0].line_editor.as_ref().unwrap().look.size
        } else {
            h.state().views[0].content.draft.as_ref().unwrap().style.size
        };
        let font = egui::FontId::proportional(size as f32 * page_scale);
        let width = h.ctx.fonts_mut(|fonts| fonts.layout_no_wrap("مرحبا".into(), font, egui::Color32::BLACK).size().x);
        let inset = if source { 0.0 } else { 4.0 };
        click(&mut h, egui::pos2(bounds.left() + inset + width / 2.0, bounds.center().y));
        let index = caret(&h);
        assert!((1..5).contains(&index), "the click lies inside the word: {index}");
        let mut expected = "مرحبا".chars().collect::<Vec<_>>();
        expected.insert(index, 'س');
        h.event(Event::Text("س".into()));
        h.run_steps(2);
        assert_eq!(text(&h, source), expected.into_iter().collect::<String>());
        if let Some(folder) = std::env::var_os("PDFCRAFT_INPUT_SNAPSHOT") {
            let name = if source { "source-input.png" } else { "added-input.png" };
            h.render().unwrap().save(std::path::Path::new(&folder).join(name)).unwrap();
        }
    }
}

/// Require the optional Arabic face for qualification, otherwise permit the bounded font-dependent case to skip.
fn arabic_font_available() -> bool {
    let present = pdfcraft_fonts::document_arabic_font().is_some();
    assert!(present || std::env::var_os("PDFCRAFT_ARABIC_CORPUS_REQUIRED").is_none(), "required craft-fonts Arabic face missing");
    present
}

#[test]
/// Verify committed multiline Arabic added text remains logical and rendered through history, saving and reopening.
fn committed_multiline_arabic_added_text_survives_history_and_save_reopen() {
    if !arabic_font_available() {
        return;
    }
    let (mut h, c) = harness(false);
    call(&mut h, &c, "ui.ime", json!({"phase":"preedit","text":"مر"})).unwrap();
    call(&mut h, &c, "ui.ime", json!({"phase":"commit","text":"مرحبا"})).unwrap();
    h.key_press(Key::Enter);
    h.run_steps(2);
    h.event(Event::Text("ABC 123".into()));
    h.run_steps(3);
    assert_eq!(text(&h, false), "مرحبا\nABC 123");
    h.get_by_label("Done adding text").click();
    h.run_steps(4);
    let id = h.state().views[0].id;
    let added_text = |app: &PdfCraftApp, id| {
        let doc = app.session.get(id).unwrap();
        let pdfcraft_engine::AddedContent::Text(text) = &doc.added()[0].content else { panic!("text") };
        text.text.clone()
    };
    assert_eq!(added_text(h.state(), id), "مرحبا\nABC 123");
    h.state_mut().execute("edit.undo");
    h.run_steps(3);
    assert!(h.state().session.get(id).unwrap().added().is_empty());
    h.state_mut().execute("edit.redo");
    h.run_steps(3);
    assert_eq!(added_text(h.state(), id), "مرحبا\nABC 123");
    let bytes = h.state().session.save_bytes(id).unwrap();
    h.state_mut().open_bytes("reopened.pdf", None, bytes.as_ref().clone()).unwrap();
    h.run_steps(3);
    let reopened = h.state().views[1].id;
    assert_eq!(added_text(h.state(), reopened), "مرحبا\nABC 123");
}

#[test]
/// Verify a native caret interruption releases composition command protection after the edit completes.
fn composition_pointer_interruption_releases_native_editor_commands() {
    for source in [true, false] {
        let (mut h, c) = harness(source);
        replace(&mut h, source, "ABC");
        select(&mut h, 3, 3);
        call(&mut h, &c, "ui.ime", json!({"phase":"preedit","text":"مر"})).unwrap();
        let committed = |h: &Harness<'static, PdfCraftApp>| {
            let ime = h.output().platform_output.ime.as_ref().expect("focused native IME owner");
            let egui::output::IMESurroundingText::Available(payload) = &ime.surrounding_text else { panic!("short native committed-text snapshot") };
            payload.text.clone()
        };
        assert_eq!(text(&h, source), "ABCمر");
        assert_eq!(committed(&h), "ABC", "live preedit is excluded from native committed text");
        let bounds = h.get_by_label(if source { "Text editing line" } else { "Added text" }).rect();
        click(&mut h, egui::pos2(bounds.left() + 2.0, bounds.center().y));
        // The native widget, not this guard, accepts the current text on caret interruption.
        // Its outgoing committed snapshot proves no live preedit remains to publish.
        assert_eq!(committed(&h), text(&h, source));
        let mut expected = text(&h, source).chars().collect::<Vec<_>>();
        expected.insert(caret(&h), '\n');
        native_input(&mut h, key_events(Key::Enter));
        assert_eq!(text(&h, source), expected.into_iter().collect::<String>(), "ordinary editor keys resume after the native interruption");
        native_input(&mut h, key_events(Key::Escape));
        assert!(if source { h.state().views[0].line_editor.is_none() } else { h.state().views[0].content.draft.is_none() });
    }
}

#[test]
/// A click elsewhere on the page closes a form field's editor and queues its text. While the
/// field holds a live preedit, that click must not write the candidate into the field (the
/// queued edit bypasses the Save-time composition check, and the focus has moved by then).
/// ASCII isolates composition ownership from optional script-font coverage.
fn form_composition_blocks_click_away_until_the_composition_ends() {
    let slot = Arc::new(Mutex::new(None));
    let capture = Arc::clone(&slot);
    let bytes = include_bytes!("data/form.pdf").to_vec();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(0.25).build_eframe(move |cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        *capture.lock().unwrap() = Some(app.attach_control(&cc.egui_ctx));
        app.open_bytes("form-ime-click.pdf", None, bytes).unwrap();
        app.set_option("left", "closed").unwrap();
        app.set_option("zoom", "150").unwrap();
        app
    });
    h.run_steps(4);
    let client = slot.lock().unwrap().take().unwrap();
    let field_value = |h: &Harness<'static, PdfCraftApp>| {
        h.state().session.get(h.state().views[0].id).unwrap().form.iter().find(|field| field.name == "name").unwrap().value.clone()
    };
    let before_value = field_value(&h);
    let field = {
        let app = h.state();
        let doc = app.session.get(app.views[0].id).unwrap();
        let field = doc.form.iter().find(|field| field.name == "name").unwrap();
        pdfcraft_ui_egui::forms_ui::field_screen_rect(&app.views[0], &doc.info, field, 0).expect("form field is visible")
    };
    let away = field.left_bottom() + egui::vec2(0.0, field.height() * 2.0);
    assert!(h.state().views[0].viewport_rect().contains(away) && !field.contains(away), "the click-away point is on the page");
    let host_click = |h: &mut Harness<'static, PdfCraftApp>| {
        h.hover_at(away);
        h.run_steps(1);
        let events = [true, false]
            .into_iter()
            .map(|pressed| Event::PointerButton { pos: away, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE })
            .collect();
        // Like real eframe hosts, run raw_input_hook before egui moves focus.
        native_input(h, events);
    };
    click(&mut h, field.center());
    let draft = h.state().views[0].forms.focus.as_ref().expect("actual form editor is open").text.clone();
    select(&mut h, 0, draft.chars().count());
    call(&mut h, &client, "ui.ime", json!({"phase":"preedit","text":"candidate"})).unwrap();
    assert_eq!(h.state().views[0].forms.focus.as_ref().unwrap().text, "candidate");
    host_click(&mut h);
    assert_eq!(field_value(&h), before_value, "clicking away cannot commit a live preedit into the field");
    let focus = h.state().views[0].forms.focus.as_ref().expect("the composing field stays open");
    assert_eq!((focus.name.as_str(), focus.text.as_str()), ("name", "candidate"));
    call(&mut h, &client, "ui.ime", json!({"phase":"commit","text":"Committed value"})).unwrap();
    host_click(&mut h);
    assert!(h.state().views[0].forms.focus.is_none(), "clicking away resumes after the composition ends");
    assert_eq!(field_value(&h), ["Committed value"], "the committed text is what the click-away writes");
}
