//! Push buttons run their actions (Reset form, Named, …) without a JavaScript engine.

use egui_kittest::Harness;
use pdfcraft_ui_egui::{Dialog, PdfCraftApp};

/// Two pages; page 1 has a text field "name" (filled), a Reset button, a Next page button and a
/// Print button (a JavaScript one-liner).
fn fixture() -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R 6 0 R 7 0 R 8 0 R] /DA (/Helv 0 Tf 0 g) >> >>",
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 400] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [5 0 R 6 0 R 7 0 R 8 0 R] >>",
        "<< /Type /Page /Parent 2 0 R >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V (Ada) /Rect [20 340 280 360] /P 3 0 R >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (reset) /Rect [20 300 100 320] /P 3 0 R /A << /S /ResetForm >> >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (next) /Rect [120 300 200 320] /P 3 0 R /A << /S /Named /N /NextPage >> >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (print) /Rect [220 300 280 320] /P 3 0 R /AA << /U << /S /JavaScript /JS (this.print\\({bUI: true}\\);) >> >> >>",
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
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("buttons.pdf", None, fixture()).unwrap();
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
    click_field(&mut h, "next");
    h.run_steps(4);
    assert_eq!(h.state().views[0].current, 1, "the NextPage action");
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
