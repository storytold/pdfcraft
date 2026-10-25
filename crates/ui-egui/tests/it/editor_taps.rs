//! A finger tap, or a pen tap on Windows, on an editor open over the page acts as a mouse click
//! there. The field editors and the Edit text paragraph editor are drawn over the pages, outside
//! the page pointer, and close on a click away. On the frame egui reports a tap there is no
//! hover position (egui-winit sends `PointerGone` with every touch release), so they must judge
//! where the click landed by the click's own position.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

use crate::support::input::touch_tap;

/// A PDF of `objects`, numbered from 1 with the catalog first.
fn pdf(objects: &[&str]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    out
}

const PAGES: &str = "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>";
const HELVETICA: &str = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>";

/// A 300×400 page with a text field `name`, a combo box `country` and a list box `colour`
/// (Red, Green, Blue) 70 pt tall.
fn form_pdf() -> Vec<u8> {
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R 6 0 R] /DA (/Helv 10 Tf 0 g) /DR << /Font << /Helv 7 0 R >> >> >> >>",
        PAGES,
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R 6 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /Rect [40 340 260 360] /P 3 0 R /F 4 >>",
        "<< /Type /Annot /Subtype /Widget /FT /Ch /Ff 131072 /T (country) /Opt [[(ca) (Canada)] [(fr) (France)] [(no) (Norway)]] /Rect [40 290 260 310] /P 3 0 R /F 4 >>",
        "<< /Type /Annot /Subtype /Widget /FT /Ch /T (colour) /Opt [(Red) (Green) (Blue)] /Rect [40 150 200 220] /P 3 0 R /F 4 >>",
        HELVETICA,
    ])
}

/// A 300×400 page with one line of 12 pt text at x 40, y 300.
fn text_pdf() -> Vec<u8> {
    let line = "BT /F1 12 Tf 40 300 Td (Signed and approved by the committee.) Tj ET";
    let content = format!("<< /Length {} >>\nstream\n{line}\nendstream", line.len() + 1);
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        PAGES,
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
        content.as_str(),
        HELVETICA,
    ])
}

/// The app with `pdf` open at 150 % and its first frames run.
fn open(pdf: Vec<u8>) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("test.pdf", None, pdf).unwrap();
        app.set_option("left", "closed").unwrap();
        app.set_option("zoom", "150").unwrap();
        app
    });
    for _ in 0..30 {
        h.run_steps(2);
    }
    h
}

/// A mouse click: move, press and release in frames of their own, and the pointer stays there.
/// (`drop_at` also sends `PointerGone` a frame later, as if the mouse left the window.)
fn mouse_click(h: &mut Harness<'_, PdfCraftApp>, p: egui::Pos2) {
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    h.run_steps(4);
}

/// Where field `name`'s widget is on screen.
fn field(h: &Harness<'_, PdfCraftApp>, name: &str) -> egui::Rect {
    widget(h, name, 0)
}

/// Where widget `w` of field `name` is on screen.
fn widget(h: &Harness<'_, PdfCraftApp>, name: &str, w: usize) -> egui::Rect {
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    let f = doc.form.iter().find(|f| f.name == name).unwrap();
    pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, f, w).expect("on screen")
}

fn value(h: &Harness<'_, PdfCraftApp>, name: &str) -> Vec<String> {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name == name).unwrap().value.clone()
}

/// The field whose editor or list is open.
fn focus(h: &Harness<'_, PdfCraftApp>) -> Option<String> {
    h.state().views[0].forms.focus.as_ref().map(|f| f.name.clone())
}

/// The Green row of the list box: the second, rows being 11.5 pt under a 2 pt inset.
fn green(h: &Harness<'_, PdfCraftApp>) -> egui::Pos2 {
    let r = field(h, "colour");
    egui::pos2(r.center().x, r.min.y + 19.0 * r.height() / 70.0)
}

// ---------------------------------------------------------------------------------------------
// Form fields.

#[test]
fn mouse_click_opens_a_combo_box() {
    // Control for the tap below.
    let mut h = open(form_pdf());
    let p = field(&h, "country").center();
    mouse_click(&mut h, p);
    assert_eq!(focus(&h).as_deref(), Some("country"), "a click opens the list of choices");
}

#[test]
fn touch_tap_opens_a_combo_box_and_picks_a_choice() {
    let mut h = open(form_pdf());
    let p = field(&h, "country").center();
    touch_tap(&mut h, p);
    assert_eq!(focus(&h).as_deref(), Some("country"), "a tap opens the list of choices, as a click does");
    let france = h.get_by_label("France").rect().center();
    touch_tap(&mut h, france);
    assert_eq!(value(&h, "country"), ["fr"], "and a tap on a choice picks it");
}

#[test]
fn mouse_click_in_a_list_box_keeps_it_focused() {
    // Control for the tap below.
    let mut h = open(form_pdf());
    let p = green(&h);
    mouse_click(&mut h, p);
    assert_eq!((value(&h, "colour"), focus(&h).as_deref()), (vec!["Green".to_string()], Some("colour")), "a click picks the row");
    h.key_press(egui::Key::ArrowDown);
    h.run_steps(3);
    assert_eq!(value(&h, "colour"), ["Blue"], "and the list keeps the keyboard");
}

#[test]
fn touch_tap_in_a_list_box_keeps_it_focused() {
    let mut h = open(form_pdf());
    let p = green(&h);
    touch_tap(&mut h, p);
    assert_eq!(
        (value(&h, "colour"), focus(&h).as_deref()),
        (vec!["Green".to_string()], Some("colour")),
        "a tap picks the row and keeps the list focused, as a click does"
    );
    h.key_press(egui::Key::ArrowDown);
    h.run_steps(3);
    assert_eq!(value(&h, "colour"), ["Blue"], "so the arrow keys go on from it");
}

#[test]
fn touch_tap_inside_an_open_text_field_keeps_typing_there() {
    // A guard more than a probe: the text editor closes when egui takes its focus away, and
    // egui judges that from the tap's own position.
    let mut h = open(form_pdf());
    let r = field(&h, "name");
    touch_tap(&mut h, r.center());
    assert_eq!(focus(&h).as_deref(), Some("name"), "a tap opens the field for typing");
    touch_tap(&mut h, r.left_center() + egui::vec2(6.0, 0.0));
    assert_eq!(focus(&h).as_deref(), Some("name"), "a tap inside the open field keeps it open");
    h.event(egui::Event::Text("Ada".into()));
    h.run_steps(1);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(value(&h, "name"), ["Ada"], "and the typing lands in it");
}

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

/// Adds a date field `due` at x 40..160, y 240..260, its calendar under it.
fn add_date_field(h: &mut Harness<'_, PdfCraftApp>) {
    h.state_mut().apply_edit(pdfcraft_engine::Edit::AddField {
        page: 0,
        rect: [40.0, 240.0, 160.0, 260.0],
        kind: pdfcraft_engine::NewField::Date,
        name: Some("due".into()),
    });
    h.run_steps(4);
}

/// Next month's calendar heading, and its 15th as the date field writes it.
fn next_month(h: &Harness<'_, PdfCraftApp>) -> (String, String) {
    let (y, m, _) = h.state().session.today();
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    (format!("{} {ny}", MONTHS[(nm - 1) as usize]), format!("{nm:02}/15/{ny}"))
}

#[test]
fn mouse_click_on_a_date_fields_calendar_keeps_it_open() {
    // Control for the tap below.
    let mut h = open(form_pdf());
    add_date_field(&mut h);
    let (heading, picked) = next_month(&h);
    let p = field(&h, "due").center();
    mouse_click(&mut h, p);
    let next = h.get_by_label("›").rect().center();
    mouse_click(&mut h, next);
    assert_eq!(focus(&h).as_deref(), Some("due"), "a click on the calendar's next month leaves the field open");
    h.get_by_label(&heading);
    let day = h.get_by_label("15").rect().center();
    mouse_click(&mut h, day);
    assert_eq!(value(&h, "due"), [picked], "and a click on a day picks it");
}

#[test]
fn touch_tap_on_a_date_fields_calendar_keeps_it_open() {
    let mut h = open(form_pdf());
    add_date_field(&mut h);
    let (heading, picked) = next_month(&h);
    let p = field(&h, "due").center();
    touch_tap(&mut h, p);
    let next = h.get_by_label("›").rect().center();
    touch_tap(&mut h, next);
    assert_eq!(focus(&h).as_deref(), Some("due"), "a tap on the calendar's next month leaves the field open, as a click does");
    h.get_by_label(&heading);
    let day = h.get_by_label("15").rect().center();
    touch_tap(&mut h, day);
    assert_eq!(value(&h, "due"), [picked], "and a tap on a day picks it");
}

/// A 300×400 page with a check box `agree` and a pair of radio buttons `pick`, A and then B to
/// its right, each 20 pt square. A click focuses them, so the keys reach them (#2223).
fn toggles_pdf() -> Vec<u8> {
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R] >> >>",
        PAGES,
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 6 0 R 7 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (agree) /V /Off /AS /Off /Rect [40 300 60 320] /P 3 0 R /F 4 /AP << /N << /Yes 8 0 R /Off 8 0 R >> >> >>",
        "<< /FT /Btn /T (pick) /Ff 32768 /V /Off /Kids [6 0 R 7 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /Parent 5 0 R /AS /Off /Rect [40 240 60 260] /P 3 0 R /F 4 /AP << /N << /A 8 0 R /Off 8 0 R >> >> >>",
        "<< /Type /Annot /Subtype /Widget /Parent 5 0 R /AS /Off /Rect [100 240 120 260] /P 3 0 R /F 4 /AP << /N << /B 8 0 R /Off 8 0 R >> >> >>",
        "<< /Type /XObject /Subtype /Form /BBox [0 0 20 20] /Length 0 >>\nstream\n\nendstream",
    ])
}

#[test]
fn mouse_click_on_a_focused_check_box_keeps_its_focus() {
    // Control for the tap below.
    let mut h = open(toggles_pdf());
    let p = field(&h, "agree").center();
    mouse_click(&mut h, p);
    assert_eq!((value(&h, "agree"), focus(&h).as_deref()), (vec!["Yes".to_string()], Some("agree")), "a click ticks and focuses the box");
    mouse_click(&mut h, p);
    assert_eq!(focus(&h).as_deref(), Some("agree"), "a click on the focused box keeps its focus");
    assert!(value(&h, "agree").is_empty(), "and clears it");
}

#[test]
fn touch_tap_on_a_focused_check_box_keeps_its_focus() {
    let mut h = open(toggles_pdf());
    let p = field(&h, "agree").center();
    mouse_click(&mut h, p);
    assert_eq!(focus(&h).as_deref(), Some("agree"), "a click focuses the box");
    touch_tap(&mut h, p);
    assert_eq!(focus(&h).as_deref(), Some("agree"), "a tap on the focused box keeps its focus, as a click does");
    assert!(value(&h, "agree").is_empty(), "and clears it");
}

#[test]
fn mouse_click_on_a_radio_button_keeps_the_group_focused() {
    // Control for the tap below.
    let mut h = open(toggles_pdf());
    let (a, b) = (widget(&h, "pick", 0).center(), widget(&h, "pick", 1).center());
    mouse_click(&mut h, a);
    assert_eq!((value(&h, "pick"), focus(&h).as_deref()), (vec!["A".to_string()], Some("pick")), "a click selects A and focuses the group");
    mouse_click(&mut h, b);
    assert_eq!((value(&h, "pick"), focus(&h).as_deref()), (vec!["B".to_string()], Some("pick")), "a click on B selects it in the focused group");
    h.key_press(egui::Key::ArrowLeft);
    h.run_steps(3);
    assert_eq!(value(&h, "pick"), ["A"], "so the arrows move within the group");
}

#[test]
fn touch_tap_on_a_radio_button_keeps_the_group_focused() {
    let mut h = open(toggles_pdf());
    let (a, b) = (widget(&h, "pick", 0).center(), widget(&h, "pick", 1).center());
    mouse_click(&mut h, a);
    assert_eq!(focus(&h).as_deref(), Some("pick"), "a click focuses the group");
    touch_tap(&mut h, b);
    assert_eq!(focus(&h).as_deref(), Some("pick"), "a tap on B keeps the group focused, as a click does");
    assert_eq!(value(&h, "pick"), ["B"], "and selects B");
    h.key_press(egui::Key::ArrowLeft);
    h.run_steps(3);
    assert_eq!(value(&h, "pick"), ["A"], "so the arrows move within the group");
}

// ---------------------------------------------------------------------------------------------
// Edit text: a click on the Format text panel keeps the paragraph open. These open the Line
// spacing list: a control that changes the paragraph on the click gives it the keyboard back in
// that same frame, which hides a click away.

/// Opens the line of `text_pdf` for typing with Edit text.
fn edit_paragraph(h: &mut Harness<'_, PdfCraftApp>) {
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(2);
    let r = h.state().views[0].page_screen_rect(0).expect("page on screen");
    let k = r.width() / 300.0;
    let p = r.min + egui::vec2(60.0 * k, (400.0 - 304.0) * k);
    mouse_click(h, p);
    assert!(h.state().views[0].line_editor.is_some(), "the paragraph opens for typing");
    assert!(h.ctx.memory(|m| m.has_focus(egui::Id::new("edit-text-line-input"))), "with the keyboard");
}

/// The open paragraph's line spacing, if a paragraph is open.
fn line_spacing(h: &Harness<'_, PdfCraftApp>) -> Option<f64> {
    h.state().views[0].line_editor.as_ref().map(|e| e.extras.line_spacing)
}

#[test]
fn mouse_click_on_format_text_keeps_the_paragraph_open() {
    // Control for the tap below.
    let mut h = open(text_pdf());
    edit_paragraph(&mut h);
    let spacing = h.get_by_value("Auto").rect().center();
    mouse_click(&mut h, spacing);
    assert_eq!(line_spacing(&h), Some(0.0), "a click opens the Line spacing list and leaves the paragraph open");
    let double = h.get_by_label("2").rect().center();
    mouse_click(&mut h, double);
    assert_eq!(line_spacing(&h), Some(2.0), "to take the spacing picked");
}

#[test]
fn touch_tap_on_format_text_keeps_the_paragraph_open() {
    let mut h = open(text_pdf());
    edit_paragraph(&mut h);
    let spacing = h.get_by_value("Auto").rect().center();
    touch_tap(&mut h, spacing);
    assert_eq!(line_spacing(&h), Some(0.0), "a tap opens the Line spacing list and leaves the paragraph open, as a click does");
    let double = h.get_by_label("2").rect().center();
    touch_tap(&mut h, double);
    assert_eq!(line_spacing(&h), Some(2.0), "to take the spacing picked");
}
