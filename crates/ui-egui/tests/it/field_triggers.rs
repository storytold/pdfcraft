//! A form field widget's pointer scripts in the app (#269, ported from #344): the pointer
//! moving onto and off a widget runs its Mouse Enter and Mouse Exit JavaScript, and a click on a
//! check box runs its Mouse Up JavaScript after the click stored the new value (#247).

use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

/// `log` records what ran; `hot` (a text field) logs the pointer entering and leaving it;
/// `strike` (a check box) shows or hides `note` from its Mouse Up script by its own new value.
fn form() -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R 6 0 R 8 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 7 0 R >> >> >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R 6 0 R 8 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (log) /V () /Rect [20 350 280 370] /P 3 0 R /F 4 >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (hot) /V () /Rect [20 250 280 290] /P 3 0 R /F 4 /AA << /E << /S /JavaScript /JS (getField\\('log'\\).value += 'in;';) >> /X << /S /JavaScript /JS (getField\\('log'\\).value += 'out;';) >> >> >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (strike) /V /Off /AS /Off /Rect [20 300 50 330] /P 3 0 R /F 4 /MK << /CA (4) >> /AP << /N << /Yes 9 0 R /Off 9 0 R >> >> /A << /S /JavaScript /JS (getField\\('note'\\).display = event.target.value == 'Off' ? display.hidden : display.visible; getField\\('log'\\).value += 'up:' + event.target.value + ';';) >> >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (note) /V (struck) /Rect [60 300 280 330] /P 3 0 R /F 6 >>",
        "<< /Type /XObject /Subtype /Form /BBox [0 0 30 30] /Length 0 >>\nstream\n\nendstream",
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

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.set_option("left", "closed").unwrap();
        app.open_bytes("triggers.pdf", None, form()).unwrap();
        app
    });
    crate::support::settle(&mut h);
    h
}

fn field<'a>(app: &'a PdfCraftApp, name: &str) -> &'a pdfcraft_engine::FormField {
    let id = app.active_ids().unwrap().1;
    app.session.get(id).unwrap().form.iter().find(|f| f.name == name).unwrap()
}

fn centre(h: &Harness<'static, PdfCraftApp>, name: &str) -> egui::Pos2 {
    let app = h.state();
    let (index, id) = app.active_ids().unwrap();
    let doc = app.session.get(id).unwrap();
    pdfcraft_ui_egui::forms_ui::field_screen_rect(&app.views[index], &doc.info, field(app, name), 0).unwrap().center()
}

fn log(h: &Harness<'static, PdfCraftApp>) -> String {
    field(h.state(), "log").value.join(",")
}

#[test]
fn the_pointer_entering_and_leaving_a_widget_runs_its_scripts() {
    let mut h = harness();
    let hot = centre(&h, "hot");
    let away = hot + egui::vec2(0.0, 200.0);
    h.hover_at(away);
    h.run_steps(2);
    h.hover_at(hot);
    crate::support::wait_until(&mut h, "Mouse Enter ran", |app| field(app, "log").value.join(",") == "in;");
    // Moving within the widget runs nothing more.
    h.hover_at(hot + egui::vec2(10.0, 0.0));
    h.run_steps(3);
    assert_eq!(log(&h), "in;");
    h.hover_at(away);
    crate::support::wait_until(&mut h, "Mouse Exit ran", |app| field(app, "log").value.join(",") == "in;out;");
}

/// The check box's Mouse Up reads the value its click stored: checking it shows the note,
/// unchecking hides it again (#247).
#[test]
fn a_check_box_mouse_up_script_sees_the_value_its_click_stored() {
    let mut h = harness();
    let strike = centre(&h, "strike");
    let click = |h: &mut Harness<'static, PdfCraftApp>| {
        h.hover_at(strike);
        h.run_steps(1);
        h.drag_at(strike);
        h.run_steps(1);
        h.drop_at(strike);
        h.run_steps(1);
    };
    click(&mut h);
    crate::support::wait_until(&mut h, "checked and its Mouse Up ran", |app| field(app, "log").value.join(",") == "up:Yes;");
    assert_eq!(field(h.state(), "strike").value, ["Yes"]);
    assert!(!field(h.state(), "note").widgets[0].hidden, "shown by the script");
    h.run_steps(6);
    click(&mut h);
    crate::support::wait_until(&mut h, "unchecked and its Mouse Up ran", |app| field(app, "log").value.join(",") == "up:Yes;up:Off;");
    assert!(field(h.state(), "note").widgets[0].hidden, "hidden again");
}
