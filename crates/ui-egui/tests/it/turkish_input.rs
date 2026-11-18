//! Turkish input acceptance. Paste events model delivery to egui; the Windows
//! clipboard and physical keyboard are separate native acceptance checks.
use egui::{Event, Key, Modifiers, Pos2};
use egui_kittest::{Harness, kittest::Queryable};
use pdfcraft_engine::{AddedContent, DocId, Session};
use pdfcraft_ui_egui::PdfCraftApp;

const TEXT: &str = "ç Ç ğ Ğ ı I i İ ö Ö ş Ş ü Ü â Â î Î û Û";
const NFD: &str = "I\u{307}stanbul C\u{327}ag\u{306}rı S\u{327}is\u{327}li GU\u{308}NES\u{327}";

fn fixture() -> Vec<u8> {
    let body = "BT /F1 16 Tf 50 520 Td (Seed paragraph) Tj ET";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] /DA (/F1 14 Tf 0 g) /DR << /Font << /F1 5 0 R >> >> >> >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 800 600] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R] /Contents 6 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (turkish) /Rect [40 420 760 470] /P 3 0 R /F 4 >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
    ];
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend(format!("{} 0 obj\n{object}\nendobj\n", i + 1).bytes());
    }
    let xref = pdf.len();
    pdf.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
    for offset in offsets {
        pdf.extend(format!("{offset:010} 00000 n \n").bytes());
    }
    pdf.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).bytes());
    pdf
}

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(0.25).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.system_clipboard = false;
        app.open_bytes("İş sözleşmesi – Şubat.pdf", None, fixture()).unwrap();
        app.set_option("left", "closed").unwrap();
        app.set_option("zoom", "80").unwrap();
        app
    });
    crate::support::settle(&mut h);
    h
}

fn click(h: &mut Harness<'static, PdfCraftApp>, at: Pos2) {
    h.hover_at(at);
    h.run_steps(1);
    h.drag_at(at);
    h.run_steps(1);
    h.drop_at(at);
    h.run_steps(4);
}

fn point(h: &Harness<'static, PdfCraftApp>, x: f32, y: f32) -> Pos2 {
    let r = h.state().views[0].page_screen_rect(0).unwrap();
    r.min + egui::vec2(x / 800.0 * r.width(), y / 600.0 * r.height())
}

fn reopened(h: &Harness<'static, PdfCraftApp>) -> (Session, DocId) {
    let app = h.state();
    let bytes = app.session.save_bytes(app.views[0].id).unwrap();
    let mut session = Session::new();
    let id = session.open("İş sözleşmesi – Şubat.pdf", Some("Çalışmalar/İş sözleşmesi – Şubat.pdf".into()), bytes, None).unwrap();
    (session, id)
}

fn added_text(h: &Harness<'static, PdfCraftApp>) -> Vec<String> {
    let app = h.state();
    app.session
        .get(app.views[0].id)
        .unwrap()
        .added()
        .iter()
        .filter_map(|a| match &a.content {
            AddedContent::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn turkish_input_added_text_paste_scalar_typing_backspace_undo_and_reopen() {
    let mut h = harness();
    assert!(h.state_mut().execute("edit.text"));
    h.run_steps(4);
    let at = point(&h, 80.0, 300.0);
    click(&mut h, at);
    h.event(Event::Paste(TEXT.into()));
    h.run_steps(4);
    for c in " Şubat".chars() {
        h.event(Event::Text(c.to_string()));
        h.run_steps(2);
    }
    h.event(Event::Text("ğ".into()));
    h.run_steps(2);
    h.key_press(Key::Backspace);
    h.run_steps(3);
    let expected = format!("{TEXT} Şubat");
    assert_eq!(h.state().views[0].content.draft.as_ref().unwrap().text, expected);
    h.get_by_label("Done adding text").click();
    h.run_steps(4);
    assert_eq!(added_text(&h).as_slice(), std::slice::from_ref(&expected));
    assert!(h.state_mut().execute("edit.undo"));
    h.run_steps(3);
    assert!(added_text(&h).is_empty());
    assert!(h.state_mut().execute("edit.redo"));
    h.run_steps(3);
    assert_eq!(added_text(&h).as_slice(), std::slice::from_ref(&expected));
    let (session, id) = reopened(&h);
    let AddedContent::Text(text) = &session.get(id).unwrap().added()[0].content else { panic!("text expected") };
    assert_eq!(text.text, expected);
}

#[test]
fn turkish_input_imported_paragraph_paste_and_reopen() {
    let mut h = harness();
    assert!(h.state_mut().execute("edit.edit_text"));
    crate::support::settle(&mut h);
    let at = point(&h, 80.0, 74.0);
    click(&mut h, at);
    assert_eq!(h.state().views[0].line_editor.as_ref().unwrap().text, "Seed paragraph");
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.event(Event::Paste(TEXT.into()));
    h.run_steps(4);
    assert_eq!(h.state().views[0].line_editor.as_ref().unwrap().text, TEXT);
    assert!(h.state_mut().execute("view.read_mode"));
    h.run_steps(4);
    let (session, id) = reopened(&h);
    let paragraphs = session.get(id).unwrap().text_blocks(0);
    // The current editor keeps different font runs on the same paragraph row.
    assert_eq!(paragraphs.iter().map(|block| block.editor_text.as_str()).collect::<Vec<_>>(), [TEXT]);
}

#[test]
fn turkish_input_form_paste_commit_undo_redo_and_reopen() {
    let mut h = harness();
    let at = {
        let app = h.state();
        let doc = app.session.get(app.views[0].id).unwrap();
        let field = doc.form.iter().find(|f| f.name == "turkish").unwrap();
        pdfcraft_ui_egui::forms_ui::field_screen_rect(&app.views[0], &doc.info, field, 0).unwrap().center()
    };
    click(&mut h, at);
    h.event(Event::Paste(TEXT.into()));
    h.run_steps(4);
    assert_eq!(h.state().views[0].forms.focus.as_ref().unwrap().text, TEXT);
    h.key_press(Key::Enter);
    h.run_steps(4);
    let value = |h: &Harness<'static, PdfCraftApp>| {
        let app = h.state();
        app.session.get(app.views[0].id).unwrap().form[0].value.clone()
    };
    assert_eq!(value(&h), [TEXT]);
    assert!(h.state_mut().execute("edit.undo"));
    h.run_steps(3);
    assert!(value(&h).is_empty());
    assert!(h.state_mut().execute("edit.redo"));
    h.run_steps(3);
    assert_eq!(value(&h), [TEXT]);
    let (session, id) = reopened(&h);
    assert_eq!(session.get(id).unwrap().form[0].value, [TEXT]);
}

#[test]
fn turkish_input_fill_sign_multiline_paste_and_reopen() {
    let mut h = harness();
    assert!(h.state_mut().execute("sign.fill.text"));
    h.run_steps(3);
    let at = point(&h, 80.0, 300.0);
    click(&mut h, at);
    let expected = format!("{TEXT}\nİstanbul Iğdır Şişli");
    h.event(Event::Paste(expected.clone()));
    h.run_steps(4);
    assert_eq!(h.state().views[0].fill_text.as_ref().unwrap().text, expected);
    click(&mut h, egui::pos2(500.0, 850.0));
    let (session, id) = reopened(&h);
    assert!(session.get(id).unwrap().info.annotations.iter().any(|a| a.subtype == "FreeText" && a.contents.as_deref() == Some(expected.as_str())));
}

#[test]
fn nfd_input_added_text_keeps_source_after_paste_undo_redo_and_reopen() {
    let mut h = harness();
    assert!(h.state_mut().execute("edit.text"));
    h.run_steps(4);
    let at = point(&h, 80.0, 300.0);
    click(&mut h, at);
    h.event(Event::Paste(NFD.into()));
    h.run_steps(4);
    assert_eq!(h.state().views[0].content.draft.as_ref().unwrap().text, NFD);
    h.get_by_label("Done adding text").click();
    h.run_steps(4);
    assert_eq!(added_text(&h), [NFD]);
    assert!(h.state_mut().execute("edit.undo"));
    h.run_steps(3);
    assert!(added_text(&h).is_empty());
    assert!(h.state_mut().execute("edit.redo"));
    h.run_steps(3);
    let (session, id) = reopened(&h);
    let AddedContent::Text(text) = &session.get(id).unwrap().added()[0].content else { panic!("text expected") };
    assert_eq!(text.text, NFD);
}

#[test]
fn nfd_input_form_and_fill_sign_keep_exact_values_after_reopen() {
    let mut h = harness();
    let at = {
        let app = h.state();
        let doc = app.session.get(app.views[0].id).unwrap();
        let field = doc.form.iter().find(|f| f.name == "turkish").unwrap();
        pdfcraft_ui_egui::forms_ui::field_screen_rect(&app.views[0], &doc.info, field, 0).unwrap().center()
    };
    click(&mut h, at);
    h.event(Event::Paste(NFD.into()));
    h.run_steps(4);
    h.key_press(Key::Enter);
    h.run_steps(4);
    assert!(h.state_mut().execute("sign.fill.text"));
    h.run_steps(3);
    let at = point(&h, 80.0, 300.0);
    click(&mut h, at);
    h.event(Event::Paste(NFD.into()));
    h.run_steps(4);
    click(&mut h, egui::pos2(500.0, 850.0));
    let (session, id) = reopened(&h);
    let doc = session.get(id).unwrap();
    assert_eq!(doc.form[0].value, [NFD]);
    assert!(doc.info.annotations.iter().any(|a| a.subtype == "FreeText" && a.contents.as_deref() == Some(NFD)));
}

fn limited_form(maximum: usize, initial: &str) -> Harness<'static, PdfCraftApp> {
    let mut h = harness();
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::SetFieldProps {
        name: "turkish".into(),
        props: Box::new(pdfcraft_engine::FieldProps { max_len: Some(Some(maximum)), ..Default::default() }),
    }));
    if !initial.is_empty() {
        assert!(
            h.state_mut().apply_edit(pdfcraft_engine::Edit::SetFieldValue {
                name: "turkish".into(),
                value: pdfcraft_engine::FieldValue::Text(initial.into()),
            })
        );
    }
    h.run_steps(3);
    let at = {
        let app = h.state();
        let doc = app.session.get(app.views[0].id).unwrap();
        let field = doc.form.iter().find(|f| f.name == "turkish").unwrap();
        pdfcraft_ui_egui::forms_ui::field_screen_rect(&app.views[0], &doc.info, field, 0).unwrap().center()
    };
    click(&mut h, at);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h
}

fn form_draft<'a>(h: &'a Harness<'static, PdfCraftApp>) -> &'a str {
    &h.state().views[0].forms.focus.as_ref().expect("a refused draft stays editable").text
}

fn form_value(h: &Harness<'static, PdfCraftApp>) -> Vec<String> {
    let app = h.state();
    app.session.get(app.views[0].id).unwrap().form[0].value.clone()
}

#[test]
fn nfd_input_max_len_preserves_all_input_channels_and_refused_drafts() {
    const CLUSTER: &str = "g\u{306}";
    for channel in ["paste", "text", "scalar-typing", "accesskit-set", "accesskit-replace", "ime"] {
        let mut h = limited_form(1, "A");
        match channel {
            "paste" => h.event(Event::Paste(CLUSTER.into())),
            "text" => h.event(Event::Text(CLUSTER.into())),
            "scalar-typing" => {
                h.event(Event::Text("g".into()));
                h.run_steps(2);
                h.event(Event::Text("\u{306}".into()));
            }
            "accesskit-set" => h.get_by_label("turkish").set_text_value_accesskit(CLUSTER),
            "accesskit-replace" => h.get_by_label("turkish").replace_selected_text_accesskit(CLUSTER),
            "ime" => {
                // A legal one-scalar preedit must not turn an over-limit NFD commit into g.
                h.event(Event::Ime(egui::ImeEvent::Preedit { text: "g".into(), active_range_chars: Some(0..1) }));
                h.run_steps(2);
                h.event(Event::Ime(egui::ImeEvent::Commit(CLUSTER.into())));
            }
            _ => unreachable!(),
        }
        h.run_steps(3);
        assert_eq!(form_draft(&h), CLUSTER, "{channel}: input must not be truncated or normalized before validation");
        h.key_press(Key::Enter);
        h.run_steps(3);
        assert_eq!(form_value(&h), ["A"], "{channel}: the PDF scalar maximum still rejects the two-scalar value");
        assert_eq!(form_draft(&h), CLUSTER, "{channel}: the complete refused NFD draft remains available");
        assert!(
            h.state().toast.as_ref().is_some_and(|message| message.0.contains("at most 1 characters")),
            "{channel}: validation explains the limit"
        );
        let (session, id) = reopened(&h);
        assert_eq!(session.get(id).unwrap().form[0].value, ["A"], "{channel}: rejected input cannot enter the saved document");
    }
}

#[test]
fn nfd_input_max_len_keeps_legacy_plain_prefix_and_accepts_complete_cluster_at_limit() {
    let mut h = limited_form(3, "");
    h.event(Event::Paste("ABCDE".into()));
    h.run_steps(3);
    assert_eq!(form_draft(&h), "ABC", "plain partial insertion keeps its existing scalar limit");
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert_eq!(form_value(&h), ["ABC"]);

    let mut h = limited_form(2, "");
    h.event(Event::Paste("g\u{306}".into()));
    h.run_steps(3);
    assert_eq!(form_draft(&h), "g\u{306}");
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert_eq!(form_value(&h), ["g\u{306}"]);
    let (session, id) = reopened(&h);
    assert_eq!(session.get(id).unwrap().form[0].value, ["g\u{306}"]);
}

#[test]
fn nfd_input_max_len_refused_draft_survives_save_without_writing_the_old_value() {
    let mut h = limited_form(1, "A");
    h.event(Event::Paste("g\u{306}".into()));
    h.run_steps(3);
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert_eq!(form_draft(&h), "g\u{306}");
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = (0..64)
        .find_map(|_| {
            let next = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("pdfcraft-nfd-maxlen-{}-{next}", std::process::id()));
            match std::fs::create_dir(&path) {
                Ok(()) => Some(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(error) => panic!("create isolated test directory: {error}"),
            }
        })
        .expect("available isolated test directory");
    let out = dir.join("refused.pdf");
    h.state_mut().save_override = Some(out.to_string_lossy().into_owned());
    h.key_press_modifiers(Modifiers::COMMAND, Key::S);
    h.run_steps(4);
    assert!(!out.exists(), "Save must stop on the invalid full NFD draft rather than writing A");
    assert_eq!(form_value(&h), ["A"]);
    assert_eq!(form_draft(&h), "g\u{306}");
    std::fs::remove_dir(&dir).unwrap();
}
