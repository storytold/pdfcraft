//! Unit tests for the sign tabs on empty signature fields (`area.rs`).

use super::area::{SIGN_TAB_MAX, sign_tab_layout, sign_tab_on_page};
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
        da: String::new(),
        quadding: 0,
        tooltip: None,
        widgets: vec![FormWidget {
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

/// Whether two layout sizes match, to a hundredth of a point.
fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.01
}

/// What a field `width` by `height` points on screen shows, for a label 30 pt wide on the
/// largest tab: "label" (the full tab), "icon" (the square chip) or "none".
fn shows(width: f32, height: f32) -> &'static str {
    match sign_tab_layout(Rect::from_min_size(Pos2::ZERO, vec2(width, height)), 30.0) {
        Some(tab) if tab.label.is_some() => "label",
        Some(_) => "icon",
        None => "none",
    }
}

/// The tab is 0.4 of the field's height on screen, at most 18 pt: a tall, wide field gets the
/// 18 pt tab with its label 3 pt inside the corner, and a 29 pt field (a 22 pt box at fit
/// width) an 11.6 pt icon chip, too small for the label, with the inset and icon scaled to it.
#[test]
fn sign_tab_layout_scales_the_chip_with_the_field_height() {
    let large = Rect::from_min_size(pos2(100.0, 200.0), vec2(400.0, 60.0));
    let tab = sign_tab_layout(large, 30.0).expect("fits");
    assert!(near(tab.rect.height(), 18.0) && tab.rect.min == large.min + vec2(3.0, 3.0), "{tab:?}");
    assert!(near(tab.icon.width(), 12.0) && tab.rect.contains_rect(tab.icon), "{tab:?}");
    let at = tab.label.expect("the label fits");
    assert!(at.x >= tab.icon.right() && at.x + 30.0 <= tab.rect.right() && at.y == tab.rect.center().y, "{tab:?}");

    let short = Rect::from_min_size(pos2(100.0, 200.0), vec2(400.0, 29.0));
    let tab = sign_tab_layout(short, 30.0).expect("fits");
    assert!(near(tab.rect.height(), 11.6) && near(tab.rect.width(), 11.6) && tab.label.is_none(), "{tab:?}");
    let inset = tab.rect.min - short.min;
    assert!(near(inset.x, 11.6 / 6.0) && near(inset.y, 11.6 / 6.0) && short.contains_rect(tab.rect), "{tab:?}");
    assert!(near(tab.icon.width(), 11.6 * 2.0 / 3.0) && tab.rect.contains_rect(tab.icon), "{tab:?}");
}

/// A 19 pt field (a 14 pt box at fit width) still gets the smallest chip, 10 pt with the icon
/// alone. A 12 pt field is too short for even that, and a rectangle that isn't finite gets
/// nothing. A label that can't be measured is left out instead of drawn at a NaN position.
#[test]
fn sign_tab_layout_keeps_a_small_icon_on_short_fields_and_none_below() {
    let field = Rect::from_min_size(pos2(100.0, 200.0), vec2(400.0, 19.0));
    let tab = sign_tab_layout(field, 30.0).expect("the smallest chip fits");
    assert!(near(tab.rect.height(), 10.0) && near(tab.rect.width(), 10.0) && tab.label.is_none(), "{tab:?}");
    assert!(field.contains_rect(tab.rect) && tab.rect.contains_rect(tab.icon), "{tab:?}");
    let cases = [
        ("12 pt tall", Rect::from_min_size(Pos2::ZERO, vec2(400.0, 12.0))),
        ("infinite", Rect::from_min_max(Pos2::ZERO, pos2(f32::INFINITY, 40.0))),
        ("NaN", Rect::from_min_max(pos2(f32::NAN, 0.0), pos2(400.0, 40.0))),
    ];
    let laid_out: Vec<&str> = cases.iter().filter(|(_, r)| sign_tab_layout(*r, 30.0).is_some()).map(|c| c.0).collect();
    assert_eq!(laid_out, Vec::<&str>::new());
    let roomy = Rect::from_min_size(Pos2::ZERO, vec2(400.0, 60.0));
    assert_eq!(sign_tab_layout(roomy, f32::NAN).map(|t| t.label), Some(None));
}

/// The label's font and the chip's corners shrink with the tab: 11 pt and 4 pt on the largest
/// tab, 8.8 pt and 3.2 pt on the 14.4 pt tab of a 36 pt field.
#[test]
fn sign_tab_layout_scales_the_label_font_and_corners_with_the_chip() {
    let large = sign_tab_layout(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 60.0)), 30.0).expect("fits");
    assert!(near(large.font, 11.0) && near(large.radius, 4.0), "{large:?}");
    let small = sign_tab_layout(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 36.0)), 30.0).expect("fits");
    assert!(near(small.font, 8.8) && near(small.radius, 3.2) && small.label.is_some(), "{small:?}");
}

/// As a 36 pt field (a 14.4 pt tab, its label 24 pt wide at that size) narrows, the tab keeps
/// its label while the whole tab fits in half the width (91.2 pt), then shows the icon alone
/// while the chip does (28.8 pt), then nothing.
#[test]
fn sign_tab_layout_drops_the_label_then_the_tab_as_the_field_narrows() {
    let widths = [400.0, 92.0, 90.0, 29.0, 28.0];
    assert_eq!(widths.map(|w| shows(w, 36.0)), ["label", "label", "icon", "icon", "none"]);
    let tab = sign_tab_layout(Rect::from_min_size(Pos2::ZERO, vec2(92.0, 36.0)), 30.0).expect("fits");
    assert!(near(tab.rect.height(), 14.4) && tab.rect.width() <= 46.0, "{tab:?}");
}

/// A field that straddles the page's edge gets its tab in the part on the page, where clicks
/// reach it, or none when that part has no room for one.
#[test]
fn a_sign_tab_stays_on_the_page() {
    let page = Rect::from_min_size(pos2(100.0, 50.0), vec2(600.0, 800.0));
    let straddling = Rect::from_min_size(pos2(60.0, 20.0), vec2(300.0, 90.0));
    let tab = sign_tab_on_page(straddling, page, 30.0).expect("the part on the page has room");
    assert!(page.contains_rect(tab.rect), "{tab:?} on {page:?}");
    assert_eq!(tab.rect.height(), SIGN_TAB_MAX, "sized to the 60 px on the page");
    let sliver = Rect::from_min_size(pos2(60.0, 20.0), vec2(300.0, 36.0));
    assert_eq!(sign_tab_on_page(sliver, page, 30.0), None, "6 px on the page");
    let off = Rect::from_min_size(pos2(60.0, 900.0), vec2(300.0, 90.0));
    assert_eq!(sign_tab_on_page(off, page, 30.0), None, "wholly off the page");
}

/// Where two sign tabs overlap, a click takes the one painted last, which is on top.
#[test]
fn the_sign_tab_painted_last_takes_the_click() {
    let tab = |x: f32| sign_tab_layout(Rect::from_min_size(pos2(x, 0.0), vec2(300.0, 60.0)), 30.0).expect("fits");
    let tabs = [("Approver", tab(0.0)), ("Witness", tab(10.0))];
    let both = tabs[1].1.rect.left_center() + vec2(2.0, 0.0);
    assert!(tabs[0].1.rect.contains(both), "the tabs overlap there");
    assert_eq!(crate::forms_ui::tab_under(&tabs, both), Some("Witness"));
    assert_eq!(crate::forms_ui::tab_under(&tabs, tabs[0].1.rect.left_center() + vec2(2.0, 0.0)), Some("Approver"));
    assert_eq!(crate::forms_ui::tab_under(&tabs, pos2(-5.0, -5.0)), None);
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

/// Every shape painted in the last frame, with nested lists flattened.
fn painted(h: &egui_kittest::Harness<'static, PdfCraftApp>) -> Vec<egui::Shape> {
    let mut stack: Vec<egui::Shape> = h.output().shapes.iter().map(|c| c.shape.clone()).collect();
    let mut out = Vec::new();
    while let Some(shape) = stack.pop() {
        match shape {
            egui::Shape::Vec(inner) => stack.extend(inner),
            shape => out.push(shape),
        }
    }
    out
}

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
fn shell(bytes: &'static [u8]) -> egui_kittest::Harness<'static, PdfCraftApp> {
    let mut h = egui_kittest::Harness::builder().with_size(vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("sign-fields.pdf", None, bytes.to_vec()).unwrap();
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "none").unwrap();
        app
    });
    h.run_steps(6);
    h
}

/// The accent chips painted in the last frame: the sign tabs.
fn chips(h: &egui_kittest::Harness<'static, PdfCraftApp>) -> Vec<Rect> {
    let accent = Tokens::get(&h.ctx).accent;
    painted(h)
        .iter()
        .filter_map(|s| match s {
            egui::Shape::Rect(r) if r.fill == accent => Some(r.rect),
            _ => None,
        })
        .collect()
}

/// Clicks at `at` and lets the shell answer.
fn click_at(h: &mut egui_kittest::Harness<'static, PdfCraftApp>, at: Pos2) {
    h.drag_at(at);
    h.run_steps(1);
    h.drop_at(at);
    h.run_steps(3);
}

/// In the real shell, an empty signature field shows its accent tab with "Sign" in its
/// top-left corner, and the signed and read-only fields show none. The tab is part of the
/// field: hovering it shows the pointing hand, and clicking it starts signing that field.
#[test]
fn empty_signature_fields_show_a_sign_tab_that_signs_them() {
    let mut h = shell(SIGN_FIELDS);
    let (shapes, chips) = (painted(&h), chips(&h));
    let (approver, witness, locked) = (field_on_screen(&h, "Approver"), field_on_screen(&h, "Witness"), field_on_screen(&h, "Locked"));
    let tab = chips.iter().copied().find(|c| approver.contains_rect(*c)).unwrap_or_else(|| panic!("no tab in {approver:?}: {chips:?}"));
    assert!(tab.min.x - approver.min.x < 8.0 && tab.min.y - approver.min.y < 8.0, "in the top-left corner: {tab:?} in {approver:?}");
    let label = shapes.iter().any(|s| matches!(s, egui::Shape::Text(t) if t.galley.text() == "Sign" && tab.contains(t.pos)));
    assert!(label, "the tab says Sign");
    assert!(!chips.iter().any(|c| witness.intersects(*c)), "a signed field has no tab: {chips:?}");
    assert!(!chips.iter().any(|c| locked.intersects(*c)), "a read-only field has no tab: {chips:?}");

    h.hover_at(tab.center());
    h.run_steps(1);
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::PointingHand);
    click_at(&mut h, tab.center());
    assert_eq!(h.state().dialog, Some(crate::Dialog::Sign));
    assert_eq!(h.state().sign_draft.as_ref().and_then(|d| d.field.clone()).as_deref(), Some("Approver"));
}

/// A sign tab is drawn over every field, so a click on it signs its field even where a field
/// earlier in the form's order overlaps it, which would otherwise take the click. Beside the
/// tab, the overlapping field still takes clicks as before.
#[test]
fn a_sign_tab_signs_its_field_where_an_earlier_field_overlaps_it() {
    let mut h = shell(SIGN_UNDER_TEXT);
    let (name, approver) = (field_on_screen(&h, "Name"), field_on_screen(&h, "Approver"));
    let tab = chips(&h).into_iter().find(|c| approver.contains_rect(*c)).expect("Approver has a tab");
    assert!(name.contains_rect(tab), "the text field covers the tab: {tab:?} in {name:?}");

    h.hover_at(tab.center());
    h.run_steps(1);
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::PointingHand);
    click_at(&mut h, tab.center());
    assert_eq!(h.state().dialog, Some(crate::Dialog::Sign));
    assert_eq!(h.state().sign_draft.as_ref().and_then(|d| d.field.clone()).as_deref(), Some("Approver"));
    assert!(h.state().views[0].forms.focus.is_none(), "the text field didn't open");

    let beside = pos2(tab.right() + 6.0, tab.center().y);
    assert!(name.contains(beside) && approver.contains(beside), "beside the tab, both fields are under the pointer");
    h.state_mut().dialog = None;
    h.state_mut().sign_draft = None;
    h.run_steps(1);
    click_at(&mut h, beside);
    assert_eq!(h.state().views[0].forms.focus.as_ref().map(|f| f.name.as_str()), Some("Name"), "the earlier field takes the click");
}
