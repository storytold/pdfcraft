//! Unit tests for the sign tabs on empty signature fields (`area.rs`).

use super::area::{sign_tab_layout, sign_tab_on_page, sign_tabs};
use super::*;
use pdfcraft_engine::{FormField, FormFieldKind, FormWidget, SignatureInfo};

/// The widget rectangle the unit tests use (user space).
const RECT: [f64; 4] = [20.0, 110.0, 140.0, 180.0];

/// An unsigned, visible signature listing for field `name` with its widget on `page` at
/// `rect`. Each test changes the one thing it is about.
fn listing(name: &str, page: usize, rect: [f64; 4]) -> SignatureInfo {
    SignatureInfo {
        field: name.into(),
        signed: false,
        page: Some(page),
        rect: Some(rect),
        visible: true,
        signer: None,
        certificate: None,
        chain: Vec::new(),
        date: None,
        signing_time: None,
        reason: None,
        location: None,
        contact: None,
        sub_filter: None,
        doc_timestamp: false,
        certify: None,
        revision: 1,
        signed_len: 0,
        digest: None,
        algorithm: None,
        timestamp: false,
        timestamp_time: None,
        status: pdfcraft_engine::SignatureStatus::Unknown,
        modification: pdfcraft_engine::sign::Modification::None,
        details: Vec::new(),
        verdict_shown: false,
    }
}

/// The signature form field `name` with one shown widget on `page` at `rect`.
fn field(name: &str, page: usize, rect: [f64; 4]) -> FormField {
    FormField {
        name: name.into(),
        obj: pdfcraft_cos::ObjRef::new(4, 0),
        kind: FormFieldKind::Signature,
        value: Vec::new(),
        default: Vec::new(),
        flags: 0,
        max_len: None,
        options: Vec::new(),
        selected: Vec::new(),
        single_value_is_array: false,
        da: String::new(),
        quadding: 0,
        tooltip: None,
        widgets: vec![FormWidget {
            mouse_up_script: None,
            mouse_down: false,
            obj: pdfcraft_cos::ObjRef::new(4, 0),
            page: Some(page),
            rect,
            on_state: None,
            state: None,
            tab: 0,
            locked: false,
            hidden: false,
            read_only: false,
            rotation: 0,
            caption: None,
            list: None,
            round: false,
        }],
        actions: Default::default(),
        button: None,
    }
}

/// The cases (by name) whose single listing and form still got a tab on page 0.
fn tabbed<'a>(cases: &[(&'a str, SignatureInfo, Vec<FormField>, bool)]) -> Vec<&'a str> {
    cases.iter().filter(|(_, s, form, can_fill)| !sign_tabs(std::slice::from_ref(s), form, 0, *can_fill).is_empty()).map(|c| c.0).collect()
}

/// An empty, visible signature field that a click signs gets one tab, at its widget, on its
/// own page only.
#[test]
fn sign_tabs_mark_an_empty_field_that_a_click_signs() {
    let approver = [listing("Approver", 0, RECT)];
    assert_eq!(sign_tabs(&approver, &[field("Approver", 0, RECT)], 0, true), vec![("Approver", RECT)]);
    let later = [listing("Witness", 1, RECT)];
    assert_eq!(sign_tabs(&later, &[field("Witness", 1, RECT)], 1, true), vec![("Witness", RECT)]);
    assert!(sign_tabs(&later, &[field("Witness", 1, RECT)], 0, true).is_empty(), "not on the page before");
}

/// Signed, invisible, other-page and unplaced fields get no tab. Each case is checked on its
/// own, so a failure names every case that wrongly got one.
#[test]
fn sign_tabs_skip_signed_invisible_other_page_and_unplaced_fields() {
    let form = || vec![field("Approver", 0, RECT)];
    let cases = [
        ("signed", SignatureInfo { signed: true, ..listing("Approver", 0, RECT) }, form(), true),
        ("invisible", SignatureInfo { visible: false, ..listing("Approver", 0, RECT) }, form(), true),
        ("on another page", listing("Approver", 1, RECT), vec![field("Approver", 1, RECT)], true),
        ("on no page", SignatureInfo { page: None, ..listing("Approver", 0, RECT) }, form(), true),
        ("without a rectangle", SignatureInfo { rect: None, ..listing("Approver", 0, RECT) }, form(), true),
    ];
    assert_eq!(tabbed(&cases), Vec::<&str>::new());
}

/// NaN, infinite and gigantic rectangles never reach the painter, even when the form's
/// widget carries the same numbers.
#[test]
fn sign_tabs_skip_rectangles_that_cannot_reach_the_screen() {
    let hostile = |name: &'static str, rect: [f64; 4]| (name, listing("Approver", 0, rect), vec![field("Approver", 0, rect)], true);
    let cases = [
        hostile("NaN", [f64::NAN, 110.0, 140.0, 180.0]),
        hostile("infinite", [20.0, f64::INFINITY, 140.0, 180.0]),
        hostile("minus infinite", [20.0, 110.0, f64::NEG_INFINITY, 180.0]),
        hostile("huge", [20.0, 110.0, 1e300, 180.0]),
    ];
    assert_eq!(tabbed(&cases), Vec::<&str>::new());
}

/// Fields a click would not sign get no tab: a read-only field, a document that refuses
/// filling, a listing with no form field of its name, a field of another kind of that name,
/// and a widget that is hidden, read-only on its own or elsewhere.
#[test]
fn sign_tabs_skip_fields_that_a_click_cannot_sign() {
    let empty = || listing("Approver", 0, RECT);
    let with = |change: fn(&mut FormField)| {
        let mut f = field("Approver", 0, RECT);
        change(&mut f);
        vec![f]
    };
    let cases = [
        ("read-only", empty(), with(|f| f.flags = pdfcraft_engine::field_flags::READ_ONLY), true),
        ("filling refused", empty(), with(|_| {}), false),
        ("no form field", empty(), Vec::new(), true),
        ("another kind", empty(), with(|f| f.kind = FormFieldKind::Text), true),
        ("hidden widget", empty(), with(|f| f.widgets.iter_mut().for_each(|w| w.hidden = true)), true),
        ("read-only widget", empty(), with(|f| f.widgets.iter_mut().for_each(|w| w.read_only = true)), true),
        ("widget elsewhere", empty(), with(|f| f.widgets.iter_mut().for_each(|w| w.rect = [160.0, 110.0, 280.0, 180.0])), true),
    ];
    assert_eq!(tabbed(&cases), Vec::<&str>::new());
}

/// What a field `width` by `height` points on screen shows, for a label 30 pt wide: 2 for the
/// full tab with its label, 1 for the square icon chip, 0 for no tab.
fn shows(width: f32, height: f32) -> u8 {
    match sign_tab_layout(Rect::from_min_size(Pos2::ZERO, vec2(width, height)), 30.0) {
        Some(tab) if tab.label.is_some() => 2,
        Some(_) => 1,
        None => 0,
    }
}

/// A non-finite field never gets a tab, and a label that can't be measured is left out instead of
/// drawn at a NaN position: a hostile file's rectangle must not reach the painter.
#[test]
fn sign_tab_layout_keeps_a_small_icon_on_short_fields_and_none_below() {
    let cases = [
        ("infinite", Rect::from_min_max(Pos2::ZERO, pos2(f32::INFINITY, 40.0))),
        ("NaN", Rect::from_min_max(pos2(f32::NAN, 0.0), pos2(400.0, 40.0))),
    ];
    let laid_out: Vec<&str> = cases.iter().filter(|(_, r)| sign_tab_layout(*r, 30.0).is_some()).map(|c| c.0).collect();
    assert_eq!(laid_out, Vec::<&str>::new());
    let roomy = Rect::from_min_size(Pos2::ZERO, vec2(400.0, 60.0));
    assert_eq!(sign_tab_layout(roomy, f32::NAN).map(|t| t.label), Some(None));
}

/// As a field narrows the tab gives up its label first, then the tab itself: what a field shows
/// never grows when it gets narrower, and runs from the label through the icon chip to nothing.
#[test]
fn sign_tab_layout_drops_the_label_then_the_tab_as_the_field_narrows() {
    let widths = [400.0, 200.0, 100.0, 60.0, 40.0, 20.0, 5.0];
    let shown: Vec<u8> = widths.iter().map(|w| shows(*w, 36.0)).collect();
    assert!(shown.windows(2).all(|pair| pair[0] >= pair[1]), "narrower never shows more: {shown:?}");
    assert_eq!((shown.first(), shown.last()), (Some(&2), Some(&0)), "from the label to nothing: {shown:?}");
    assert!(shown.contains(&1), "the icon chip lies between: {shown:?}");
}

/// A field that straddles the page's edge gets its tab in the part on the page, where clicks
/// reach it, or none when that part has no room for one.
#[test]
fn a_sign_tab_stays_on_the_page() {
    let page = Rect::from_min_size(pos2(100.0, 50.0), vec2(600.0, 800.0));
    let straddling = Rect::from_min_size(pos2(60.0, 20.0), vec2(300.0, 90.0));
    let tab = sign_tab_on_page(straddling, page, 30.0).expect("the part on the page has room");
    assert!(page.contains_rect(tab.rect), "{tab:?} on {page:?}");
    let sliver = Rect::from_min_size(pos2(60.0, 20.0), vec2(300.0, 36.0));
    assert_eq!(sign_tab_on_page(sliver, page, 30.0), None, "6 px on the page");
    let off = Rect::from_min_size(pos2(60.0, 900.0), vec2(300.0, 90.0));
    assert_eq!(sign_tab_on_page(off, page, 30.0), None, "wholly off the page");
}

/// One page with three signature fields: Approver is empty, Witness is signed (a signature
/// dictionary as its value is enough to list it as signed) and Locked is empty but read-only.
const SIGN_FIELDS: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R 6 0 R] >> >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Annots [4 0 R 5 0 R 6 0 R] >> endobj
4 0 obj << /Type /Annot /Subtype /Widget /FT /Sig /T (Approver) /Rect [20 110 140 180] /P 3 0 R /F 4 >> endobj
5 0 obj << /Type /Annot /Subtype /Widget /FT /Sig /T (Witness) /V << /Type /Sig >> /Rect [160 110 280 180] /P 3 0 R /F 4 >> endobj
6 0 obj << /Type /Annot /Subtype /Widget /FT /Sig /T (Locked) /Ff 1 /Rect [20 20 140 90] /P 3 0 R /F 4 >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// Where the named field's first widget is on screen.
fn field_on_screen(h: &egui_kittest::Harness<'static, PdfCraftApp>, name: &str) -> Rect {
    let app = h.state();
    let view = &app.views[0];
    let doc = app.session.get(view.id).expect("open");
    let f = doc.form.iter().find(|f| f.name == name).expect("field");
    crate::forms_ui::field_screen_rect(view, &doc.info, f, 0).expect("on screen")
}

/// One page where the text field Name, first in the form's order, covers the top-left corner
/// of the empty signature field Approver, where its sign tab is drawn.
const SIGN_UNDER_TEXT: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R] >> >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Annots [4 0 R 5 0 R] >> endobj
4 0 obj << /Type /Annot /Subtype /Widget /FT /Tx /T (Name) /Rect [10 120 130 190] /P 3 0 R /F 4 >> endobj
5 0 obj << /Type /Annot /Subtype /Widget /FT /Sig /T (Approver) /Rect [20 110 140 180] /P 3 0 R /F 4 >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// The real shell with `bytes` open, the side panes closed, after the first frames.
fn shell(ctx: &egui::Context, bytes: &'static [u8]) -> egui_kittest::Harness<'static, PdfCraftApp> {
    let mut h = egui_kittest::Harness::builder().with_size(vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::PdfCraftApp::install_runtime(&cc.egui_ctx);
        {
            let mut app = PdfCraftApp::for_unit_tests();
            app.set_option(&cc.egui_ctx, "language", "en").unwrap();
            app.open_bytes(ctx, "sign-fields.pdf", None, bytes.to_vec()).unwrap();
            app.set_option(&cc.egui_ctx, "left", "closed").unwrap();
            app.set_option(&cc.egui_ctx, "panel", "none").unwrap();
            app
        }
    });
    h.run_steps(6);
    h
}

/// Where the sign tab of the named field is on screen: the chip in the corner of the part of its
/// widget on the first page, which is where a click on it signs. A label's width only widens the
/// chip to the right, so the icon chip (no label) is the part of the tab that is there at every width.
fn tab_on_screen(h: &egui_kittest::Harness<'static, PdfCraftApp>, name: &str) -> Rect {
    let page = h.state().views[0].page_xform(0).expect("page 0 is on screen").rect;
    sign_tab_on_page(field_on_screen(h, name), page, 0.0).expect("the field has room for a tab").rect
}

/// Clicks at `at` and lets the shell answer.
fn click_at(h: &mut egui_kittest::Harness<'static, PdfCraftApp>, at: Pos2) {
    h.drag_at(at);
    h.run_steps(1);
    h.drop_at(at);
    h.run_steps(3);
}

/// In the real shell, an empty signature field's sign tab is part of the field: hovering it shows
/// the pointing hand, and clicking it starts signing that field. The signed and read-only fields
/// have no tab: a click where theirs would be starts no signing.
#[test]
fn empty_signature_fields_show_a_sign_tab_that_signs_them() {
    let ctx = &egui::Context::default();
    let mut h = shell(ctx, SIGN_FIELDS);
    let (witness, locked) = (tab_on_screen(&h, "Witness"), tab_on_screen(&h, "Locked"));
    for (what, spot) in [("a signed field", witness), ("a read-only field", locked)] {
        click_at(&mut h, spot.center());
        assert!(h.state().sign_draft.is_none(), "{what} has no tab that signs it");
    }

    let tab = tab_on_screen(&h, "Approver");
    h.hover_at(tab.center());
    h.run_steps(1);
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::PointingHand);
    click_at(&mut h, tab.center());
    assert_eq!(h.state().sign_draft.as_ref().and_then(|d| d.field.clone()).as_deref(), Some("Approver"));
}

/// A sign tab is over every field, so a click on it signs its field even where a field
/// earlier in the form's order overlaps it, which would otherwise take the click. Beside the
/// tab, the overlapping field still takes clicks as before.
#[test]
fn a_sign_tab_signs_its_field_where_an_earlier_field_overlaps_it() {
    let ctx = &egui::Context::default();
    let mut h = shell(ctx, SIGN_UNDER_TEXT);
    let (name, approver) = (field_on_screen(&h, "Name"), field_on_screen(&h, "Approver"));
    let tab = tab_on_screen(&h, "Approver");
    assert!(name.contains_rect(tab), "the text field covers the tab: {tab:?} in {name:?}");

    h.hover_at(tab.center());
    h.run_steps(1);
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::PointingHand);
    click_at(&mut h, tab.center());
    assert_eq!(h.state().sign_draft.as_ref().and_then(|d| d.field.clone()).as_deref(), Some("Approver"));
    assert!(h.state().views[0].session.forms.focus.is_none(), "the text field didn't open");

    // Three quarters across Approver, level with the tab: clear of the tab at any label width, and
    // under both fields.
    let beside = pos2(approver.min.x + approver.width() * 0.75, tab.center().y);
    assert!(name.contains(beside) && approver.contains(beside), "beside the tab, both fields are under the pointer");
    // The sign dialog closes, and the signing the click started is forgotten.
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(!h.state().modal_open(), "the sign dialog is closed");
    h.state_mut().sign_draft = None;
    click_at(&mut h, beside);
    assert_eq!(h.state().views[0].session.forms.focus.as_ref().map(|f| f.name.as_str()), Some("Name"), "the earlier field takes the click");
}
