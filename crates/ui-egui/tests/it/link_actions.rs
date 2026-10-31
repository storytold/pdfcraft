//! Links run the named (ISO 32000-2 §12.6.4.12), hide (§12.6.4.11) and reset-form (§12.7.6.3)
//! actions, and the actions their `/Next` entries chain (§12.6.2), as push buttons run them.

use egui::Pos2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{Dialog, PdfCraftApp};

/// Link rows on page 1, by the y (PDF user space, 300 × 400 page) of their middle.
const NEXT: f32 = 375.0;
const LAST: f32 = 350.0;
const GO_BACK: f32 = 325.0;
const PRINT: f32 = 300.0;
const BOGUS: f32 = 275.0;
const HIDE: f32 = 250.0;
const SHOW: f32 = 225.0;
const RESET_F1: f32 = 200.0;
const RESET_ALL_BUT_F1: f32 = 175.0;
const NEXT_THEN_GO_TO_4: f32 = 150.0;
const RESET_THEN_HIDE: f32 = 125.0;
/// Every other page has one link at this height: PrevPage on page 2, FirstPage on page 3,
/// NextPage on page 4 (the last).
const ROW: f32 = 375.0;

/// Four pages. Page 1 has three text fields at the bottom ("secret", "f1" and "f2", each typed
/// over its default) and a link per action; pages 2–4 one link each.
fn fixture() -> Vec<u8> {
    let link =
        |y: f32, action: &str| format!("<< /Type /Annot /Subtype /Link /Rect [20 {} 280 {}] /Border [0 0 0] /A {action} >>", y - 10.0, y + 10.0);
    let field = |name: &str, x: u32, v: &str, dv: &str| {
        format!(
            "<< /Type /Annot /Subtype /Widget /FT /Tx /F 4 /T ({name}) /V ({v}) /DV ({dv}) /Rect [{x} 20 {} 40] /P 3 0 R /DA (/Helv 10 Tf 0 g) >>",
            x + 80
        )
    };
    let page = |annots: &str| format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Annots [{annots}] >>");
    // Objects 1–7: catalog, pages, the four pages; 8–10 the fields; 11 on the page-1 links;
    // then the links of pages 2, 3 and 4.
    let page1_links = [
        (NEXT, "<< /S /Named /N /NextPage >>"),
        (LAST, "<< /S /Named /N /LastPage >>"),
        (GO_BACK, "<< /S /Named /N /GoBack >>"),
        (PRINT, "<< /S /Named /N /Print >>"),
        (BOGUS, "<< /S /Named /N /Bogus >>"),
        (HIDE, "<< /S /Hide /T (secret) >>"),
        (SHOW, "<< /S /Hide /T 8 0 R /H false >>"),
        (RESET_F1, "<< /S /ResetForm /Fields [(f1)] /Flags 0 >>"),
        (RESET_ALL_BUT_F1, "<< /S /ResetForm /Fields [(f1)] /Flags 1 >>"),
        (NEXT_THEN_GO_TO_4, "<< /S /Named /N /NextPage /Next << /S /GoTo /D [6 0 R /Fit] >> >>"),
        (RESET_THEN_HIDE, "<< /S /ResetForm /Fields [(f2)] /Next [<< /S /Hide /T (secret) >>] >>"),
    ];
    let first_link = 11;
    let refs = |from: usize, n: usize| (from..from + n).map(|k| format!("{k} 0 R")).collect::<Vec<_>>().join(" ");
    let after = first_link + page1_links.len();
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [8 0 R 9 0 R 10 0 R] /DA (/Helv 10 Tf 0 g) >> >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R 6 0 R] /Count 4 >>".to_string(),
        page(&format!("8 0 R 9 0 R 10 0 R {}", refs(first_link, page1_links.len()))),
        page(&refs(after, 1)),
        page(&refs(after + 1, 1)),
        page(&refs(after + 2, 1)),
        "<< >>".to_string(),
        field("secret", 20, "SECRET", "SECRET"),
        field("f1", 110, "typed 1", "one"),
        field("f2", 200, "typed 2", "two"),
    ];
    objs.extend(page1_links.iter().map(|(y, a)| link(*y, a)));
    objs.push(link(ROW, "<< /S /Named /N /PrevPage >>"));
    objs.push(link(ROW, "<< /S /Named /N /FirstPage >>"));
    objs.push(link(ROW, "<< /S /Named /N /NextPage >>"));
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
        app.open_bytes("actions.pdf", None, fixture()).unwrap();
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "none").unwrap();
        app.set_option("zoom", "100").unwrap();
        app
    });
    h.run_steps(8);
    h
}

/// The screen position of (150, y) on `page`, given in PDF user space.
fn at(h: &Harness<'static, PdfCraftApp>, page: usize, y: f32) -> Pos2 {
    let r = h.state().views[0].page_screen_rect(page).expect("the page is on screen");
    let k = r.width() / 300.0;
    r.min + egui::vec2(150.0 * k, (400.0 - y) * k)
}

/// Click the link at height `y` on the current page.
fn click(h: &mut Harness<'static, PdfCraftApp>, y: f32) {
    let page = h.state().views[0].current;
    let p = at(h, page, y);
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(4);
}

fn current(h: &Harness<'static, PdfCraftApp>) -> usize {
    h.state().views[0].current
}

fn value(h: &Harness<'static, PdfCraftApp>, name: &str) -> String {
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    doc.form.iter().find(|f| f.name == name).map(|f| f.value.join("")).unwrap_or_default()
}

/// Whether the "secret" widget (object 8) has the Hidden annotation flag (§12.5.3) once saved.
fn secret_hidden(h: &Harness<'static, PdfCraftApp>) -> bool {
    let s = h.state();
    let bytes = s.session.save_bytes(s.views[0].id).unwrap();
    let doc = pdfcraft_cos::Document::open(bytes).unwrap();
    let f = doc.get(pdfcraft_cos::ObjRef::new(8, 0)).as_dict().and_then(|d| d.int(b"F")).unwrap_or(0);
    f & 2 != 0
}

fn dirty(h: &Harness<'static, PdfCraftApp>) -> bool {
    h.state().session.get(h.state().views[0].id).unwrap().dirty
}

fn toast(h: &Harness<'static, PdfCraftApp>) -> Option<String> {
    h.state().toast.clone().map(|t| t.0)
}

/// Table 215's four names go where they say; NextPage on the last page and PrevPage on the first
/// stay put, as a push button's do.
#[test]
fn links_run_the_standard_named_actions() {
    let mut h = harness();
    click(&mut h, NEXT);
    assert_eq!(current(&h), 1, "NextPage");
    click(&mut h, ROW);
    assert_eq!(current(&h), 0, "PrevPage");
    click(&mut h, LAST);
    assert_eq!(current(&h), 3, "LastPage");
    click(&mut h, ROW);
    assert_eq!(current(&h), 3, "NextPage on the last page does nothing");
    h.state_mut().views[0].go_to_page(2);
    h.run_steps(4);
    click(&mut h, ROW);
    assert_eq!(current(&h), 0, "FirstPage");
    assert_eq!(toast(&h), None, "no action was reported as unsupported");
    assert!(!dirty(&h), "moving around changes nothing in the document");
}

/// "If the PDF processor ... does not recognise the name, it shall take no action": no
/// navigation, no edit and no message.
#[test]
fn an_unknown_named_action_does_nothing() {
    let mut h = harness();
    click(&mut h, BOGUS);
    assert_eq!(current(&h), 0);
    assert_eq!(toast(&h), None);
    assert_eq!(h.state().dialog, None);
    assert!(!dirty(&h));
}

/// The nonstandard GoBack goes back to the previous view (nothing without one), and Print opens
/// the Print dialog.
#[test]
fn go_back_returns_to_the_previous_view_and_print_opens_the_dialog() {
    let mut h = harness();
    click(&mut h, GO_BACK);
    assert_eq!(current(&h), 0, "no previous view: nothing happens");
    assert_eq!(toast(&h), None);
    click(&mut h, PRINT);
    assert_eq!(h.state().dialog, Some(Dialog::Print), "Print opens the dialog");
    h.state_mut().dialog = None;
    h.run_steps(2);
    // NextPage to page 2, then PrevPage back to page 1: page 2 is the previous view.
    click(&mut h, NEXT);
    assert_eq!(current(&h), 1);
    click(&mut h, ROW);
    assert_eq!(current(&h), 0);
    click(&mut h, GO_BACK);
    assert_eq!(current(&h), 1, "GoBack returns to the previous view");
    assert_eq!(h.state().views[0].forward, [0], "as View ▸ Previous view does: page 1 is the next view");
}

/// A hide action sets and clears the field's Hidden flag as the push-button one does: an
/// undoable edit that marks the document changed.
#[test]
fn links_hide_and_show_fields_undoably() {
    let mut h = harness();
    assert!(!secret_hidden(&h));
    click(&mut h, HIDE);
    assert!(secret_hidden(&h), "the hide action hid the field");
    assert!(dirty(&h));
    click(&mut h, SHOW);
    assert!(!secret_hidden(&h), "/H false (by a widget reference) showed it again");
    assert_eq!(toast(&h), None, "nothing was reported as unsupported");
    h.state_mut().execute("edit.undo");
    h.run_steps(3);
    assert!(secret_hidden(&h), "undo hides it again");
}

/// A reset-form action resets the listed fields, or all but them with the Include/Exclude flag.
#[test]
fn links_reset_listed_or_all_but_listed_fields() {
    let mut h = harness();
    click(&mut h, RESET_F1);
    assert_eq!((value(&h, "f1"), value(&h, "f2"), value(&h, "secret")), ("one".into(), "typed 2".into(), "SECRET".into()));
    h.state_mut().execute("edit.undo");
    h.run_steps(3);
    assert_eq!(value(&h, "f1"), "typed 1", "undoable");
    click(&mut h, RESET_ALL_BUT_F1);
    assert_eq!((value(&h, "f1"), value(&h, "f2")), ("typed 1".into(), "two".into()));
    assert!(dirty(&h));
}

/// `/Next` actions run after the link's own, in order: NextPage then GoTo page 4 ends on page
/// 4, and a reset then a hide are one undoable step.
#[test]
fn links_run_next_action_chains() {
    let mut h = harness();
    click(&mut h, RESET_THEN_HIDE);
    assert_eq!(value(&h, "f2"), "two");
    assert!(secret_hidden(&h));
    h.state_mut().execute("edit.undo");
    h.run_steps(3);
    assert_eq!(value(&h, "f2"), "typed 2", "one undo takes back both");
    assert!(!secret_hidden(&h));
    click(&mut h, NEXT_THEN_GO_TO_4);
    assert_eq!(current(&h), 3, "NextPage, then the chained GoTo");
}

/// The old half of a side-by-side comparison is not the active tab, but a link clicked there
/// resets its own document's field at once, not when that tab later comes to the front.
#[test]
fn a_link_in_the_old_half_of_a_comparison_edits_its_document_at_once() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("old.pdf", None, fixture()).unwrap();
        app.open_bytes("new.pdf", None, fixture()).unwrap();
        app.set_option("zoom", "100").unwrap();
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("doc.compare"));
    h.run_steps(2);
    h.get_by_label("Compare").click();
    h.run_steps(4);
    let c = h.state().compare.as_ref().expect("comparing");
    assert!(c.side_by_side);
    let (old_id, new_id) = (c.old, c.new);
    let old = h.state().views.iter().position(|v| v.id == old_id).unwrap();
    let active = h.state().active;
    let r = h.state().views[old].page_screen_rect(0).expect("the old page is on screen");
    let k = r.width() / 300.0;
    let p = r.min + egui::vec2(150.0 * k, (400.0 - RESET_F1) * k);
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(4);
    assert_eq!(h.state().active, active, "the click did not change the active tab");
    let f1 = |h: &Harness<'static, PdfCraftApp>, id| {
        let doc = h.state().session.get(id).unwrap();
        doc.form.iter().find(|f| f.name == "f1").map(|f| f.value.join("")).unwrap_or_default()
    };
    assert_eq!(f1(&h, old_id), "one", "the old document's field was reset");
    assert_eq!(f1(&h, new_id), "typed 1", "the new document is untouched");
    assert!(h.state().views[old].pending_edit.is_none(), "nothing is left waiting on the old tab");
}
