//! Push buttons run their actions (Reset form, Named, …) without a JavaScript engine.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{Dialog, PdfCraftApp};

/// Two pages; page 1 has a text field "name" (filled), a Reset button, a Next page button and a
/// Print button (a JavaScript one-liner).
fn fixture() -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R 6 0 R 7 0 R 8 0 R 9 0 R] /DA (/Helv 0 Tf 0 g) >> >>",
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 400] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [5 0 R 6 0 R 7 0 R 8 0 R 9 0 R] >>",
        "<< /Type /Page /Parent 2 0 R >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V (Ada) /Rect [20 340 280 360] /P 3 0 R >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (reset) /Rect [20 300 100 320] /P 3 0 R /A << /S /ResetForm >> >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (next) /Rect [120 300 200 320] /P 3 0 R /A << /S /Named /N /NextPage >> >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (print) /Rect [220 300 280 320] /P 3 0 R /AA << /U << /S /JavaScript /JS (this.print\\({bUI: true}\\);) >> >> >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (save) /Rect [20 260 100 280] /P 3 0 R /A << /S /Named /N /Save >> >>",
    ];
    pdf(&objs)
}

fn pdf(objs: &[impl AsRef<str>]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", i + 1, o.as_ref()).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn click_field(h: &mut Harness<'static, PdfCraftApp>, name: &str) {
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

#[test]
fn buttons_reset_navigate_and_print() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-buttons-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("buttons.pdf");
    std::fs::write(&path, fixture()).unwrap();
    let path_str = path.to_string_lossy().into_owned();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("buttons.pdf", Some(path_str), fixture()).unwrap();
        app.set_option("zoom", "150").unwrap();
        app
    });
    h.run_steps(6);
    click_field(&mut h, "reset");
    {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        assert!(doc.form.iter().find(|f| f.name == "name").unwrap().value.is_empty(), "Reset form cleared the field");
    }
    click_field(&mut h, "print");
    assert_eq!(h.state().dialog, Some(Dialog::Print), "this.print() opens the Print dialog");
    h.state_mut().dialog = None;
    h.run_steps(2);
    // A document's Save named action never writes the file unasked (PdfCraft's XFA buttons
    // use SaveAs, which opens the Save As dialog, as a script's `execMenuItem` does): the
    // reset above made the document dirty, and the click leaves it so.
    let before = std::fs::read(&path).unwrap();
    click_field(&mut h, "save");
    h.run_steps(4);
    h.get_by_label_contains("isn't supported");
    assert!(h.state().session.get(h.state().views[0].id).unwrap().dirty, "not saved");
    assert_eq!(std::fs::read(&path).unwrap(), before, "the file is untouched");
    click_field(&mut h, "next");
    h.run_steps(4);
    assert_eq!(h.state().views[0].current, 1, "the NextPage action");
}

/// Same-named leaves and similarly prefixed groups expose reset actions that lose ancestors.
fn qualified_reset_fixture(target: &str, exclude: bool) -> Vec<u8> {
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 10 0 R 13 0 R 14 0 R] /DA (/Helv 10 Tf 0 g) /DR << /Font << /Helv 15 0 R >> >> >> >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [6 0 R 7 0 R 9 0 R 12 0 R 13 0 R 14 0 R] >>".into(),
        "<< /T (billing) /Kids [5 0 R 8 0 R] >>".into(),
        "<< /T (person) /Parent 4 0 R /Kids [6 0 R 7 0 R] >>".into(),
        "<< /Subtype /Widget /FT /Tx /T (name) /Parent 5 0 R /V (bill) /DV (bill-default) /Rect [20 340 280 360] /AP << /N 18 0 R >> /PCMarker (kept) >>".into(),
        "<< /Subtype /Widget /FT /Tx /T (code) /Parent 5 0 R /V (code) /DV (code-default) /Rect [20 310 280 330] /AP << /N 19 0 R >> /PCMarker (kept) >>".into(),
        "<< /T (personExtra) /Parent 4 0 R /Kids [9 0 R] >>".into(),
        "<< /Subtype /Widget /FT /Tx /T (name) /Parent 8 0 R /V (extra) /DV (extra-default) /Rect [20 280 280 300] /AP << /N 20 0 R >> /PCMarker (kept) >>".into(),
        "<< /T (shipping) /Kids [11 0 R] >>".into(),
        "<< /T (person) /Parent 10 0 R /Kids [12 0 R] >>".into(),
        "<< /Subtype /Widget /FT /Tx /T (name) /Parent 11 0 R /V (ship) /DV (ship-default) /Rect [20 250 280 270] /AP << /N 21 0 R >> /PCMarker (kept) >>".into(),
        "<< /Subtype /Widget /FT /Tx /T (name) /V (root) /DV (root-default) /Rect [20 220 280 240] /AP << /N 22 0 R >> /PCMarker (kept) >>".into(),
        format!("<< /Subtype /Widget /FT /Btn /Ff 65536 /T (reset) /Rect [20 180 100 200] /A << /S /ResetForm /Fields [{target}] /Flags {} >> >>", i32::from(exclude)),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        "(billing.person.name)".into(),
        "(billing.person)".into(),
    ];
    for value in ["bill", "code", "extra", "ship", "root"] {
        let content = format!("BT /Helv 10 Tf 0 5 Td ({value}) Tj ET\n");
        objs.push(format!(
            "<< /Type /XObject /Subtype /Form /BBox [0 0 260 20] /Resources << /Font << /Helv 15 0 R >> >> /Length {} >>\nstream\n{content}endstream",
            content.len()
        ));
    }
    pdf(&objs)
}

#[test]
fn reset_buttons_ignore_unresolved_explicit_targets() {
    for parent in ["99 0 R", "10 0 R"] {
        for targets in ["5 0 R", "7 0 R 5 0 R"] {
            for exclude in [false, true] {
                let objs = vec![
                    "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 6 0 R 7 0 R 8 0 R] /DA (/Helv 10 Tf 0 g) /DR << /Font << /Helv 9 0 R >> >> >> >>".to_string(),
                    "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>".into(),
                    "<< /Type /Page /Parent 2 0 R /Annots [5 0 R 6 0 R 7 0 R 8 0 R] >>".into(),
                    "<< /T (group) /Kids [5 0 R] >>".into(),
                    format!("<< /Subtype /Widget /FT /Tx /T (name) /Parent {parent} /V (target-current) /DV (target-default) /Rect [20 340 280 360] >>"),
                    "<< /Subtype /Widget /FT /Tx /T (name) /V (root-current) /DV (root-default) /Rect [20 300 280 320] >>".into(),
                    "<< /Subtype /Widget /FT /Tx /T (safe) /V (safe-current) /DV (safe-default) /Rect [20 260 280 280] >>".into(),
                    format!("<< /Subtype /Widget /FT /Btn /Ff 65536 /T (reset) /Rect [20 180 100 200] /A << /S /ResetForm /Fields [{targets}] /Flags {} >> >>", i32::from(exclude)),
                    "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
                    "(not a field dictionary)".into(),
                ];
                let bytes = pdf(&objs);
                let original_bytes = std::sync::Arc::new(bytes.clone());
                let original = pdfcraft_cos::Document::open(original_bytes.clone()).unwrap();
                let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
                    let mut app = PdfCraftApp::new();
                    app.set_option("language", "en").unwrap();
                    app.open_bytes("unresolved-reset.pdf", None, bytes).unwrap();
                    app.set_option("zoom", "150").unwrap();
                    app
                });
                h.run_steps(6);
                let id = h.state().views[0].id;
                assert_eq!(
                    h.state().session.get(id).unwrap().form.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
                    ["group.name", "name", "safe", "reset"]
                );
                click_field(&mut h, "reset");
                let live = h.state().session.get(id).unwrap();
                // Check the collision-prone root name first, before saving can affect the result.
                for (name, current, default) in [
                    ("name", "root-current", "root-default"),
                    ("safe", "safe-current", "safe-default"),
                    ("group.name", "target-current", "target-default"),
                ] {
                    let field = live.form.iter().find(|f| f.name == name).unwrap();
                    assert_eq!(field.value, [current], "live {name}, parent {parent}, targets {targets}, exclude {exclude}");
                    assert_eq!(field.default, [default], "live defaults are unchanged");
                }
                assert!(!live.dirty, "the disabled reset action must not create an edit");
                let saved = h.state().session.save_bytes(id).unwrap();
                assert_eq!(saved.as_slice(), original_bytes.as_slice(), "a disabled reset preserves the original saved bytes");
                let reopened = pdfcraft_cos::Document::open(saved.clone()).unwrap();
                let mut reopened_session = pdfcraft_engine::Session::new();
                let reopened_id = reopened_session.open("reopened.pdf", None, saved, None).unwrap();
                let after = reopened_session.get(reopened_id).unwrap();
                assert_eq!(after.form.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["group.name", "name", "safe", "reset"]);
                for (name, current, default) in [
                    ("name", "root-current", "root-default"),
                    ("safe", "safe-current", "safe-default"),
                    ("group.name", "target-current", "target-default"),
                ] {
                    let field = after.form.iter().find(|f| f.name == name).unwrap();
                    assert_eq!(field.value, [current], "saved {name}, parent {parent}, targets {targets}, exclude {exclude}");
                    assert_eq!(field.default, [default], "saved defaults are unchanged");
                }
                for num in 4..=8 {
                    let obj = pdfcraft_cos::ObjRef::new(num, 0);
                    assert_eq!(reopened.get(obj).as_dict(), original.get(obj).as_dict(), "field dictionary {num} is unchanged");
                }
            }
        }
    }
}

#[test]
fn reset_buttons_respect_qualified_field_and_group_references() {
    // Field dictionaries, direct strings and indirect strings identify the same fields.
    let expected_names = ["billing.person.name", "billing.person.code", "billing.personExtra.name", "shipping.person.name", "name"];
    for (target, group) in
        [("6 0 R", false), ("(billing.person.name)", false), ("16 0 R", false), ("5 0 R", true), ("(billing.person)", true), ("17 0 R", true)]
    {
        for exclude in [false, true] {
            let bytes = qualified_reset_fixture(target, exclude);
            let original = pdfcraft_cos::Document::open(std::sync::Arc::new(bytes.clone())).unwrap();
            let appearance = |doc: &pdfcraft_cos::Document, field: &pdfcraft_engine::FormField| {
                let widget = doc.get(field.widgets[0].obj);
                let ap = doc.resolve(widget.as_dict().unwrap().get(b"AP").unwrap());
                let normal = doc.resolve(ap.as_dict().unwrap().get(b"N").unwrap());
                let pdfcraft_cos::Object::Stream(stream) = &*normal else { panic!("normal appearance") };
                (stream.decoded().unwrap(), stream.dict.get(b"Resources").cloned())
            };
            let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
                let mut app = PdfCraftApp::new();
                app.set_option("language", "en").unwrap();
                app.open_bytes("qualified-reset.pdf", None, bytes).unwrap();
                app.set_option("zoom", "150").unwrap();
                app
            });
            h.run_steps(6);
            let original_fields = h.state().session.get(h.state().views[0].id).unwrap().form.clone();
            assert_eq!(
                original_fields.iter().filter(|f| f.kind == pdfcraft_engine::FormFieldKind::Text).map(|f| f.name.as_str()).collect::<Vec<_>>(),
                expected_names,
                "all original text fields are present"
            );
            click_field(&mut h, "reset");
            let s = h.state();
            let live = s.session.get(s.views[0].id).unwrap();
            let saved = s.session.save_bytes(s.views[0].id).unwrap();
            let reopened = pdfcraft_cos::Document::open(saved.clone()).unwrap();
            let mut reopened_session = pdfcraft_engine::Session::new();
            let reopened_id = reopened_session.open("reopened.pdf", None, saved, None).unwrap();
            let reopened_fields = &reopened_session.get(reopened_id).unwrap().form;
            assert_eq!(
                reopened_fields.iter().filter(|f| f.kind == pdfcraft_engine::FormFieldKind::Text).map(|f| f.name.as_str()).collect::<Vec<_>>(),
                expected_names,
                "all saved text fields are present"
            );
            for before in original_fields.iter().filter(|f| f.kind == pdfcraft_engine::FormFieldKind::Text) {
                let listed = before.name == "billing.person.name" || (group && before.name == "billing.person.code");
                let reset = listed != exclude;
                let expected = if reset { &before.default } else { &before.value };
                let live_field = live.form.iter().find(|f| f.name == before.name).unwrap();
                let after = reopened_fields.iter().find(|f| f.name == before.name).unwrap();
                assert_eq!(&live_field.value, expected, "live {}, target {target}, exclude {exclude}", before.name);
                assert_eq!(&after.value, expected, "saved {}, target {target}, exclude {exclude}", before.name);
                assert_eq!(after.default, before.default, "defaults are preserved");
                assert_eq!(reopened.get(after.obj).as_dict().unwrap().get(b"PCMarker"), original.get(before.obj).as_dict().unwrap().get(b"PCMarker"));
                if !reset {
                    assert_eq!(appearance(&reopened, after), appearance(&original, before), "untargeted appearances are preserved");
                }
            }
        }
    }
}

fn empty_ancestor_reset_fixture(target: &str, exclude: bool) -> Vec<u8> {
    let mut doc = pdfcraft_cos::Document::open(qualified_reset_fixture(target, exclude).into()).unwrap();
    for (obj, name) in [(4, ""), (6, "city"), (9, "extra"), (12, "ship"), (13, "other")] {
        doc.update_dict(pdfcraft_cos::ObjRef::new(obj, 0), |d| {
            d.set(b"T".to_vec(), pdfcraft_cos::Object::String(pdfcraft_cos::PdfString::text(name)));
        })
        .unwrap();
    }
    pdfcraft_cos::write_incremental(&doc, &pdfcraft_cos::SaveOptions::default()).unwrap()
}

#[test]
fn reset_buttons_match_imported_empty_ancestor_names() {
    let names = ["person.city", "person.code", "personExtra.extra", "shipping.person.ship", "other"];
    for (target, group) in [("6 0 R", false), ("5 0 R", true)] {
        for exclude in [false, true] {
            let bytes = empty_ancestor_reset_fixture(target, exclude);
            let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
                let mut app = PdfCraftApp::new();
                app.set_option("language", "en").unwrap();
                app.open_bytes("empty-ancestor-reset.pdf", None, bytes).unwrap();
                app.set_option("zoom", "150").unwrap();
                app
            });
            h.run_steps(6);
            let id = h.state().views[0].id;
            assert_eq!(
                h.state()
                    .session
                    .get(id)
                    .unwrap()
                    .form
                    .iter()
                    .filter(|f| f.kind == pdfcraft_engine::FormFieldKind::Text)
                    .map(|f| f.name.as_str())
                    .collect::<Vec<_>>(),
                names,
                "the importer retains unique names below the empty ancestor"
            );
            click_field(&mut h, "reset");
            let live = h.state().session.get(id).unwrap();
            let saved = h.state().session.save_bytes(id).unwrap();
            let mut reopened = pdfcraft_engine::Session::new();
            let reopened_id = reopened.open("reopened.pdf", None, saved, None).unwrap();
            let after = reopened.get(reopened_id).unwrap();
            assert_eq!(
                after.form.iter().filter(|f| f.kind == pdfcraft_engine::FormFieldKind::Text).map(|f| f.name.as_str()).collect::<Vec<_>>(),
                names
            );
            for (name, current, default) in [
                ("person.city", "bill", "bill-default"),
                ("person.code", "code", "code-default"),
                ("personExtra.extra", "extra", "extra-default"),
                ("shipping.person.ship", "ship", "ship-default"),
                ("other", "root", "root-default"),
            ] {
                let selected = name == "person.city" || (group && name == "person.code");
                let expected = if selected != exclude { default } else { current };
                assert_eq!(live.form.iter().find(|f| f.name == name).unwrap().value, [expected], "live {name}, target {target}, exclude {exclude}");
                let field = after.form.iter().find(|f| f.name == name).unwrap();
                assert_eq!(field.value, [expected], "saved {name}, target {target}, exclude {exclude}");
                assert_eq!(field.default, [default], "defaults are unchanged");
            }
        }
    }
}

/// One page: a text field "name" and two push buttons whose Hide actions hide it and show it again.
fn show_hide_fixture() -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R 6 0 R] /DA (/Helv 0 Tf 0 g) >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R 6 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V (Ada) /F 4 /Rect [20 340 280 360] /P 3 0 R >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (hide) /Rect [20 300 100 320] /P 3 0 R /A << /S /Hide /T (name) >> >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (show) /Rect [120 300 200 320] /P 3 0 R /A << /S /Hide /T 4 0 R /H false >> >>",
    ];
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

/// Whether the "name" widget (object 4) has the Hidden annotation flag in the saved document.
fn name_hidden(h: &Harness<'static, PdfCraftApp>) -> bool {
    let s = h.state();
    let bytes = s.session.save_bytes(s.views[0].id).unwrap();
    let doc = pdfcraft_cos::Document::open(bytes).unwrap();
    let f = doc.get(pdfcraft_cos::ObjRef::new(4, 0)).as_dict().and_then(|d| d.int(b"F")).unwrap_or(0);
    f & 2 != 0
}

#[test]
fn hide_actions_hide_and_show_fields() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("show-hide.pdf", None, show_hide_fixture()).unwrap();
        app.set_option("zoom", "150").unwrap();
        app
    });
    h.run_steps(6);
    assert!(!name_hidden(&h));
    // A push button with an action shows the pointing hand, not the "not allowed" cursor.
    let hide_at = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let f = doc.form.iter().find(|f| f.name == "hide").unwrap();
        pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, f, 0).expect("on screen").center()
    };
    h.hover_at(hide_at);
    h.run_steps(2);
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::PointingHand);
    click_field(&mut h, "hide");
    assert!(name_hidden(&h), "the Hide action hid the field");
    // A hidden field takes no clicks: clicking where it was doesn't start editing it.
    click_field(&mut h, "name");
    assert!(h.state().views[0].forms.focus.is_none(), "a hidden field can't be focused");
    click_field(&mut h, "show");
    assert!(!name_hidden(&h), "the Hide action with /H false showed it again");
}
