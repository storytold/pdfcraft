//! Prepare a form in the real shell (egui_kittest): field tools, placing, selecting, moving,
//! Field Properties and deleting.

use egui::Pos2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{PdfCraftApp, QuickTool};

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, include_bytes!("data/form.pdf").to_vec()).unwrap();
        app.set_option("zoom", "150").unwrap();
        app
    });
    for _ in 0..60 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h
}

/// A point on page 1 at (x, y) points from its top-left.
fn at(h: &Harness<'static, PdfCraftApp>, x: f32, y: f32) -> Pos2 {
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let k = r.width() / 300.0;
    r.min + egui::vec2(x * k, y * k)
}

fn drag(h: &mut Harness<'static, PdfCraftApp>, a: Pos2, b: Pos2) {
    h.hover_at(a);
    h.run_steps(1);
    h.drag_at(a);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(a + (b - a) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    h.drop_at(b);
    h.run_steps(4);
}

fn names(h: &Harness<'static, PdfCraftApp>) -> Vec<String> {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().form.iter().map(|f| f.name.clone()).collect()
}

fn rect_of(h: &Harness<'static, PdfCraftApp>, name: &str) -> [f64; 4] {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name == name).unwrap().widgets[0].rect
}

#[test]
fn placing_moving_editing_and_deleting_a_field() {
    let mut h = harness();
    let before = names(&h).len();
    assert!(h.state_mut().execute("form.add.text"));
    h.run_steps(2);
    h.get_by_label("Prepare a form");
    // Drag out a text field in the empty lower part of the page.
    let (a, b) = (at(&h, 40.0, 300.0), at(&h, 200.0, 322.0));
    drag(&mut h, a, b);
    let all = names(&h);
    assert_eq!(all.len(), before + 1, "{all:?}");
    assert_eq!(all.last().map(String::as_str), Some("Text1"));
    assert_eq!(h.state().quick_tool, QuickTool::Select, "back to Select after one field");
    h.run_steps(2);
    assert_eq!(h.state().views[0].prepare.selected.as_ref().map(|s| s.0.as_str()), Some("Text1"), "the new field is selected");
    let r = rect_of(&h, "Text1");
    assert!((r[2] - r[0] - 160.0).abs() < 2.0 && (r[3] - r[1] - 22.0).abs() < 2.0, "{r:?}");
    // Move it 20 pt right.
    let c = at(&h, 120.0, 311.0);
    let d = c + egui::vec2(at(&h, 20.0, 0.0).x - at(&h, 0.0, 0.0).x, 0.0);
    drag(&mut h, c, d);
    let m = rect_of(&h, "Text1");
    assert!((m[0] - r[0] - 20.0).abs() < 1.5, "moved: {r:?} → {m:?}");
    // Field Properties: rename and make it required (one undo step).
    h.state_mut().execute("form.field.properties");
    h.run_steps(2);
    h.get_by_label("Text Field Properties");
    {
        let d = h.state_mut().field_props.as_mut().expect("open");
        d.name = "email".into();
        d.required = true;
    }
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert!(names(&h).contains(&"email".to_string()));
    {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        assert_eq!(doc.can_undo(), Some("Change field properties"));
        assert!(doc.form.iter().find(|f| f.name == "email").unwrap().has(pdfcraft_engine::field_flags::REQUIRED));
        assert_eq!(s.views[0].prepare.selected.as_ref().map(|s| s.0.as_str()), Some("email"));
    }
    // Delete removes the selected field.
    h.key_press(egui::Key::Delete);
    h.run_steps(3);
    assert_eq!(names(&h).len(), before);
    // A click with the check box tool places a default-size one.
    h.state_mut().execute("form.add.checkbox");
    h.run_steps(1);
    let p = at(&h, 40.0, 250.0);
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(4);
    let r = rect_of(&h, "Check Box1");
    assert_eq!((r[2] - r[0], r[3] - r[1]), (14.0, 14.0));
}

#[test]
fn format_validate_and_calculate_tabs() {
    use pdfcraft_engine::form_scripts::{Calculate, Format, Validate};
    let mut h = harness();
    h.state_mut().execute("form.prepare");
    h.run_steps(2);
    h.state_mut().open_field_props("city", 0);
    h.run_steps(2);
    h.get_by_label("Format").click();
    h.run_steps(2);
    h.get_by_label("Select format category:");
    {
        let d = h.state_mut().field_props.as_mut().unwrap();
        d.format = Format::Number { decimals: 2, sep: 0, neg: 0, currency: "$".into(), prepend: true };
        d.validate = Validate::Range { min: Some(0.0), max: None };
        d.calculate = Calculate::Notation("name".into());
    }
    h.run_steps(2);
    h.get_by_label_contains("Example: -$1,234.50");
    h.get_by_label("Calculate").click();
    h.run_steps(2);
    h.get_by_label("Simplified field notation:");
    h.get_by_label("OK").click();
    h.run_steps(3);
    let s = h.state();
    let f = s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name == "city").unwrap().clone();
    assert!(matches!(f.actions.format, Format::Number { decimals: 2, .. }));
    assert_eq!(f.actions.validate, Validate::Range { min: Some(0.0), max: None });
    assert_eq!(f.actions.calculate, Calculate::Notation("name".into()));
}

#[test]
fn appearance_tab_restyles_the_field() {
    use pdfcraft_engine::{BorderStyle, FieldFont};
    let mut h = harness();
    h.state_mut().execute("form.prepare");
    h.run_steps(2);
    h.state_mut().open_field_props("city", 0);
    h.run_steps(2);
    h.get_by_label("Appearance").click();
    h.run_steps(2);
    h.get_by_label("Line Style:");
    {
        let d = h.state_mut().field_props.as_mut().unwrap();
        let l = d.look.as_mut().expect("a look");
        l.style = BorderStyle::Dashed;
        l.font = FieldFont::Courier;
        l.fill = Some([1.0, 1.0, 0.8]);
    }
    h.get_by_label("OK").click();
    h.run_steps(3);
    let s = h.state();
    let l = s.session.get(s.views[0].id).unwrap().field_look("city").unwrap();
    assert_eq!((l.style, l.font, l.fill), (BorderStyle::Dashed, FieldFont::Courier, Some([1.0, 1.0, 0.8])));
}

#[test]
fn position_tab_rotates_a_field() {
    let mut h = harness();
    h.state_mut().open_field_props("city", 0);
    h.run_steps(2);
    let widget = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        let f = s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name == "city").unwrap();
        f.widgets[0].clone()
    };
    let before = widget(&h);
    assert_eq!(before.rotation, 0);
    assert!(h.state().field_props.as_ref().unwrap().props().is_none(), "an unchanged draft is not an edit");
    h.get_by_label("Position").click();
    h.run_steps(2);
    h.get_by_label("Rotation:");
    h.state_mut().field_props.as_mut().unwrap().rotation = 90;
    assert!(h.state().field_props.as_ref().unwrap().props().is_some());
    h.get_by_label("OK").click();
    h.run_steps(3);
    let turned = widget(&h);
    assert_eq!(turned.rotation, 90);
    let [x0, y0, x1, y1] = before.rect;
    let (cx, cy, w, ht) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0, x1 - x0, y1 - y0);
    let expect = [cx - ht / 2.0, cy - w / 2.0, cx + ht / 2.0, cy + w / 2.0];
    assert!(turned.rect.iter().zip(expect).all(|(a, b)| (a - b).abs() < 0.05), "{:?} vs {expect:?}", turned.rect);
    h.state_mut().execute("edit.undo");
    h.run_steps(3);
    let back = widget(&h);
    assert_eq!(back.rotation, 0);
    assert!(back.rect.iter().zip(before.rect).all(|(a, b)| (a - b).abs() < 0.05), "{:?} vs {:?}", back.rect, before.rect);
}

#[test]
fn options_tab_sets_the_check_box_style() {
    // #94: Check Box Style on the Options tab.
    use pdfcraft_engine::CheckStyle;
    let mut h = harness();
    h.state_mut().apply_edit(pdfcraft_engine::Edit::AddField {
        page: 0,
        rect: [40.0, 300.0, 56.0, 316.0],
        kind: pdfcraft_engine::NewField::CheckBox,
        name: Some("agree".into()),
    });
    h.state_mut().open_field_props("agree", 0);
    h.run_steps(2);
    h.get_by_label("Options").click();
    h.run_steps(2);
    h.get_by_label("Check Box Style:");
    assert_eq!(h.state().field_props.as_ref().unwrap().check_style, Some(CheckStyle::Check));
    h.state_mut().field_props.as_mut().unwrap().check_style = Some(CheckStyle::Cross);
    h.get_by_label("OK").click();
    h.run_steps(3);
    let s = h.state();
    assert_eq!(s.session.get(s.views[0].id).unwrap().field_check_style("agree"), Some(CheckStyle::Cross));
}

#[test]
fn options_tab_sets_flags_alignment_and_defaults() {
    use pdfcraft_engine::field_flags as ff;
    let mut h = harness();
    h.state_mut().open_field_props("city", 0);
    h.run_steps(2);
    h.get_by_label("Options").click();
    h.run_steps(2);
    h.get_by_label("Check spelling").click();
    h.get_by_label("Password").click();
    h.run_steps(1);
    {
        let d = h.state_mut().field_props.as_mut().unwrap();
        d.quadding = 2;
        d.default = "Paris".into();
    }
    h.get_by_label("OK").click();
    h.run_steps(3);
    let s = h.state();
    let f = s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name == "city").cloned().unwrap();
    assert!(f.has(ff::PASSWORD) && f.has(ff::DO_NOT_SPELL_CHECK), "{:b}", f.flags);
    assert_eq!((f.quadding, f.default.clone()), (2, vec!["Paris".to_string()]));
    assert_eq!(s.session.get(s.views[0].id).unwrap().can_undo(), Some("Change field properties"));
}

#[test]
fn the_fields_panel_orders_tabs_manually() {
    let mut h = harness();
    h.state_mut().execute("form.prepare");
    h.state_mut().set_option("panel", "fields").unwrap();
    h.run_steps(3);
    let order = |h: &Harness<'static, PdfCraftApp>| -> Vec<String> {
        let s = h.state();
        let form = &s.session.get(s.views[0].id).unwrap().form;
        let mut v: Vec<(usize, String)> = form.iter().filter_map(|f| f.widgets.iter().map(|w| w.tab).min().map(|t| (t, f.name.clone()))).collect();
        v.sort();
        v.into_iter().map(|x| x.1).collect()
    };
    let before = order(&h);
    assert!(before.len() >= 2);
    let second = before[1].clone();
    h.get_by_label(&format!("Earlier in tab order: {second}")).click();
    h.run_steps(3);
    let after = order(&h);
    assert_eq!(after[0], second, "{before:?} → {after:?}");
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Set tab order"));
}

#[test]
fn duplicating_a_field_onto_every_page() {
    let mut h = harness();
    h.state_mut().apply_edit(pdfcraft_engine::Edit::InsertBlankPage { at: 1, width: 300.0, height: 400.0 });
    h.state_mut().execute("form.prepare");
    h.run_steps(3);
    let (n, p) = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let f = doc.form.iter().find(|f| f.name == "city").unwrap();
        (doc.info.pages.len(), pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, f, 0).expect("on screen").center())
    };
    assert!(n >= 2, "the fixture has {n} pages");
    h.hover_at(p);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed: true, modifiers: Default::default() });
    h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed: false, modifiers: Default::default() });
    h.run_steps(3);
    h.get_by_label("Duplicate…").click();
    h.run_steps(2);
    h.get_by_label("Duplicate Field");
    h.get_by_label("OK").click();
    h.run_steps(3);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    let f = doc.form.iter().find(|f| f.name == "city").unwrap();
    let mut pages: Vec<usize> = f.widgets.iter().filter_map(|w| w.page).collect();
    pages.sort();
    assert_eq!(pages, (0..n).collect::<Vec<_>>());
    assert_eq!(doc.can_undo(), Some("Duplicate field"));
}

#[test]
fn aligning_distributing_and_sizing_several_fields() {
    use pdfcraft_ui_egui::prepare::{Arrange, arrange};
    let mut h = harness();
    for (name, x, y, w) in [("a", 20.0, 300.0, 60.0), ("b", 120.0, 280.0, 80.0), ("c", 260.0, 260.0, 30.0)] {
        h.state_mut().apply_edit(pdfcraft_engine::Edit::AddField {
            page: 0,
            rect: [x, y, x + w, y + 20.0],
            kind: pdfcraft_engine::NewField::Text { multiline: false },
            name: Some(name.into()),
        });
    }
    h.run_steps(2);
    let form = |h: &Harness<'static, PdfCraftApp>| h.state().session.get(h.state().views[0].id).unwrap().form.as_ref().clone();
    let rect = |h: &Harness<'static, PdfCraftApp>, n: &str| form(h).iter().find(|f| f.name == n).unwrap().widgets[0].rect;
    let a = ("a".to_string(), 0);
    let others = vec![("b".to_string(), 0), ("c".to_string(), 0)];
    // Align tops with a.
    let e = arrange(&form(&h), &a, &others, Arrange::AlignTop).unwrap();
    h.state_mut().apply_edit(e);
    assert!(["b", "c"].iter().all(|n| rect(&h, n)[3] == 320.0));
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Align fields"));
    // Distribute horizontally: equal gaps between a, b and c.
    let e = arrange(&form(&h), &a, &others, Arrange::DistributeH).unwrap();
    h.state_mut().apply_edit(e);
    let (ra, rb, rc) = (rect(&h, "a"), rect(&h, "b"), rect(&h, "c"));
    assert!(((rb[0] - ra[2]) - (rc[0] - rb[2])).abs() < 1e-6, "{ra:?} {rb:?} {rc:?}");
    // Same width as a.
    let e = arrange(&form(&h), &a, &others, Arrange::SameWidth).unwrap();
    h.state_mut().apply_edit(e);
    assert!(["b", "c"].iter().all(|n| (rect(&h, n)[2] - rect(&h, n)[0] - 60.0).abs() < 1e-6));
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Match field sizes"));
    assert!(arrange(&form(&h), &a, &[("b".into(), 0)], Arrange::DistributeV).is_none(), "distributing needs three");
}

#[test]
fn deleting_several_selected_fields_and_detecting_fields_from_the_panel() {
    let mut h = harness();
    for (name, x) in [("a", 20.0), ("b", 120.0), ("c", 220.0)] {
        h.state_mut().apply_edit(pdfcraft_engine::Edit::AddField {
            page: 0,
            rect: [x, 300.0, x + 60.0, 320.0],
            kind: pdfcraft_engine::NewField::Text { multiline: false },
            name: Some(name.into()),
        });
    }
    assert!(h.state_mut().execute("form.prepare"));
    h.run_steps(2);
    // #93: field detection is in the panel, not only run when a form has no fields.
    h.get_by_label("Detect form fields");
    // #95: Delete removes every selected field, as one undoable step.
    {
        let p = &mut h.state_mut().views[0].prepare;
        p.selected = Some(("a".into(), 0));
        p.also = vec![("b".into(), 0), ("c".into(), 0)];
    }
    h.key_press(egui::Key::Delete);
    h.run_steps(3);
    let left = names(&h);
    assert!(!["a", "b", "c"].iter().any(|n| left.iter().any(|l| l == n)), "{left:?}");
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Delete 3 fields"));
    assert!(h.state().views[0].prepare.selected.is_none() && h.state().views[0].prepare.also.is_empty());
}

#[test]
fn preview_fills_the_form_and_locked_fields_keep_their_properties() {
    let mut h = harness();
    assert!(h.state_mut().execute("form.prepare"));
    h.run_steps(2);
    assert!(h.state().is_preparing());
    h.get_by_label("Preview").click();
    h.run_steps(2);
    assert!(!h.state().is_preparing(), "Preview fills the form");
    h.get_by_label("Edit fields").click();
    h.run_steps(2);
    assert!(h.state().is_preparing());
    // Lock a field: its properties are then greyed out until unlocked.
    h.state_mut().open_field_props("city", 0);
    h.run_steps(2);
    h.get_by_label("Locked").click();
    h.run_steps(1);
    h.get_by_label("OK").click();
    h.run_steps(3);
    let locked = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name == "city").unwrap().locked()
    };
    assert!(locked(&h));
    h.state_mut().open_field_props("city", 0);
    h.run_steps(2);
    h.get_by_label_contains("This field is locked");
    h.get_by_label("Locked").click();
    h.run_steps(1);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert!(!locked(&h));
}

#[test]
fn image_fields_take_a_picture_when_clicked() {
    let mut h = harness();
    assert!(h.state_mut().execute("form.add.image"));
    h.run_steps(2);
    let (a, b) = (at(&h, 40.0, 220.0), at(&h, 140.0, 290.0));
    drag(&mut h, a, b);
    assert_eq!(names(&h).last().map(String::as_str), Some("Image1"));
    // A picture to choose (the picker is bypassed in tests).
    let path = std::env::temp_dir().join(format!("pdfcraft-image-field-{}.png", std::process::id()));
    image::RgbImage::from_pixel(8, 4, image::Rgb([200, 30, 30])).save(&path).unwrap();
    h.state_mut().save_override = Some(path.to_string_lossy().into_owned());
    // Fill it in as a reader would.
    h.get_by_label("Preview").click();
    h.run_steps(2);
    let c = at(&h, 90.0, 255.0);
    h.hover_at(c);
    h.run_steps(1);
    h.drag_at(c);
    h.run_steps(1);
    h.drop_at(c);
    h.run_steps(4);
    let s = h.state();
    assert_eq!(s.session.get(s.views[0].id).unwrap().can_undo(), Some("Set the image of Image1"));
}

#[test]
fn actions_tab_adds_and_removes_actions() {
    use pdfcraft_engine::{FieldAction, FieldTrigger};
    let mut h = harness();
    h.state_mut().execute("form.prepare");
    h.run_steps(2);
    h.state_mut().open_field_props("city", 0);
    h.run_steps(2);
    h.get_by_label("Actions").click();
    h.run_steps(2);
    h.get_by_label("Select Trigger:");
    h.get_by_label("No actions");
    {
        let d = h.state_mut().field_props.as_mut().unwrap();
        d.new_action.trigger = FieldTrigger::OnBlur;
        d.new_action.kind = 1;
        d.new_action.text = "https://example.org".into();
    }
    h.run_steps(3);
    h.get_by_label("Add").click();
    h.run_steps(2);
    h.get_by_label("Open a web link: https://example.org");
    {
        let d = h.state_mut().field_props.as_mut().unwrap();
        d.new_action.trigger = FieldTrigger::MouseUp;
        d.new_action.kind = 0;
        d.new_action.text = "app.alert('x')".into();
    }
    h.run_steps(3);
    h.get_by_label("Add").click();
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
    let s = h.state();
    let acts = s.session.get(s.views[0].id).unwrap().field_actions("city");
    assert_eq!(
        acts,
        [
            (FieldTrigger::MouseUp, FieldAction::JavaScript("app.alert('x')".into())),
            (FieldTrigger::OnBlur, FieldAction::Uri("https://example.org".into()))
        ]
    );
}
