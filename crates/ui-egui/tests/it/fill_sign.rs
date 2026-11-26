//! Fill & Sign in the real shell (egui_kittest): text, marks, date and a drawn signature.

use std::sync::Arc;

use craft::host::{Picked, ScriptedFiles};
use craft::review::SavedSignature as SavedSig;
use craft::review::ui::signature::SignaturePadDialog;
use egui::{Pos2, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use pdfcraft_ui_egui::fill_sign::FillTool;
use pdfcraft_ui_egui::settings::import_json;
use pdfcraft_ui_egui::{PdfCraftApp, QuickTool, SaveTarget};

const FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF";

use crate::gpu;
use crate::support::AppHarness as _;

fn harness() -> Harness<'static, PdfCraftApp> {
    harness_bytes(FIXTURE)
}

fn harness_bytes(fixture: &[u8]) -> Harness<'static, PdfCraftApp> {
    harness_on(fixture, None, None)
}

/// [`harness`] on a host whose file pickers answer from `files`: queue the answer before the click that asks.
fn harness_with_files(files: &Arc<ScriptedFiles>) -> Harness<'static, PdfCraftApp> {
    harness_on(FIXTURE, Some(files.clone()), None)
}

/// [`harness`] whose document came from `out` on a host that keeps what is written in `files`: a save writes
/// `out` there without a picker, and the test reads it back with `files.file(out)`.
fn harness_saving(files: &Arc<ScriptedFiles>, out: &std::path::Path) -> Harness<'static, PdfCraftApp> {
    harness_on(FIXTURE, Some(files.clone()), Some(out.to_string_lossy().into_owned()))
}

fn harness_on(fixture: &[u8], files: Option<Arc<ScriptedFiles>>, path: Option<String>) -> Harness<'static, PdfCraftApp> {
    let fixture = fixture.to_vec();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = match &files {
                Some(files) => crate::control::app_with_files(files.clone()),
                None => crate::control::new_app(),
            };
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "form.pdf", path.clone(), fixture.clone()).unwrap();
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "zoom", "150").unwrap();
            app.set_option(&egui::Context::default(), "author", "Ada").unwrap();
            app
        }
    });
    h.run_steps(5);
    h
}

fn at(h: &Harness<'static, PdfCraftApp>, x: f32, y: f32) -> Pos2 {
    let s = h.state();
    let page = &s.session.get(s.views[0].id).unwrap().info.pages[0];
    let xf = pdfcraft_ui_egui::canvas::PageXform {
        rect: s.views[0].page_screen_rect(0).expect("on screen"),
        rot: s.views[0].view.rotation,
        pw: page.width,
        ph: page.height,
    };
    let p = page.user_to_view(x, y);
    xf.norm_to_screen(p[0] / xf.pw, p[1] / xf.ph)
}

/// The screen point `dx` points right and `dy` points up, as displayed, from the user-space point
/// (`x`, `y`): unlike `at`, offsets don't turn with the page's /Rotate.
fn shown(h: &Harness<'static, PdfCraftApp>, x: f32, y: f32, dx: f32, dy: f32) -> Pos2 {
    let s = h.state();
    let page = &s.session.get(s.views[0].id).unwrap().info.pages[0];
    let xf = pdfcraft_ui_egui::canvas::PageXform {
        rect: s.views[0].page_screen_rect(0).expect("on screen"),
        rot: s.views[0].view.rotation,
        pw: page.width,
        ph: page.height,
    };
    let p = page.user_to_view(x, y);
    xf.norm_to_screen((p[0] + dx) / xf.pw, (p[1] - dy) / xf.ph)
}

fn click(h: &mut Harness<'static, PdfCraftApp>, x: f32, y: f32) {
    let p = at(h, x, y);
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(3);
}

/// The toolbar's Fill & Sign slot, named by the command of the tool it shows (`command` is that command's label). The
/// slot's flyout mark carries the same name, inside the slot, so the slot is the first of them.
fn fill_slot<'h>(h: &'h Harness<'static, PdfCraftApp>, command: &'h str) -> egui_kittest::Node<'h> {
    h.query_all_by_role_and_label(egui::accesskit::Role::Button, command).next().unwrap_or_else(|| panic!("no toolbar slot named {command:?}"))
}

/// The row of the Fill & Sign slot's flyout that runs `command` (its label).
fn fill_flyout_row<'h>(h: &'h Harness<'static, PdfCraftApp>, command: &str) -> egui_kittest::Node<'h> {
    let command = command.to_owned();
    h.get_by(move |n| {
        matches!(n.role(), egui::accesskit::Role::MenuItem | egui::accesskit::Role::MenuItemRadio) && n.label().as_deref() == Some(command.as_str())
    })
}

fn items(h: &Harness<'static, PdfCraftApp>) -> Vec<(String, Option<String>)> {
    let s = h.state();
    let mut v: Vec<_> = s.session.get(s.views[0].id).unwrap().info.annotations.iter().map(|a| (a.subtype.clone(), a.contents.clone())).collect();
    v.sort();
    v
}

/// Whether the signature pad is open.
fn pad_open(h: &Harness<'static, PdfCraftApp>) -> bool {
    h.state().dialogs.is_open::<SignaturePadDialog>()
}

/// Whether something on screen is named `label` (a name the pad's dialog and its tabs may share).
fn shows(h: &Harness<'static, PdfCraftApp>, label: &str) -> bool {
    h.query_all_by_label(label).next().is_some()
}

/// Whether the pad's Apply button is enabled: the draft holds something to save.
fn can_apply(h: &Harness<'static, PdfCraftApp>) -> bool {
    !h.get_by_label("Apply").accesskit_node().is_disabled()
}

/// The saved settings written and read back into `app`, as a restart does.
fn restore_from(app: &mut PdfCraftApp, saved: &PdfCraftApp) {
    app.apply_settings(&saved.settings_snapshot());
}

/// `json`, a legacy settings text, applied to `app` as an import does.
fn restore_json(app: &mut PdfCraftApp, json: &str) {
    app.apply_settings(&import_json(json).0);
}

fn undo(h: &mut Harness<'static, PdfCraftApp>) {
    let ctx = h.ctx.clone();
    h.state_mut().undo(&ctx);
}

fn redo(h: &mut Harness<'static, PdfCraftApp>) {
    let ctx = h.ctx.clone();
    h.state_mut().redo(&ctx);
}

/// Turn the view a quarter turn clockwise.
fn turn_view(h: &mut Harness<'static, PdfCraftApp>) {
    let view = &mut h.state_mut().views[0].view;
    view.rotation = (view.rotation + 90) % 360;
}

/// Page rasters arrive from the render worker independently of kittest's virtual frames.
fn rendered_ink(h: &mut Harness<'static, PdfCraftApp>, x: f32, y: f32) -> image::RgbaImage {
    for _ in 0..50 {
        h.run_steps(2);
        let p = at(h, x, y);
        let pixels = h.render().unwrap();
        if pixels.get_pixel(p.x as u32, p.y as u32).0[..3].iter().all(|v| *v < 60) {
            return pixels;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("signature ink did not render at ({x}, {y})");
}

/// #2392: the Line tool's line runs across the page as displayed, on turned pages too (its box was
/// sized in the page's own axes, so a quarter turn stood it on end).
#[test]
fn the_line_mark_runs_across_turned_pages() {
    for degrees in [0, 90, 180, 270] {
        let mut h = harness();
        {
            let s = h.state_mut();
            let id = s.views[0].id;
            s.session.apply(id, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees }).unwrap();
        }
        h.run_steps(4);
        assert!(h.execute("sign.fill.line"));
        click(&mut h, 150.0, 350.0);
        let s = h.state();
        let info = &s.session.get(s.views[0].id).unwrap().info;
        let r = info.annotations.last().expect("a line was placed").rect;
        let (a, b) = (info.pages[0].user_to_view(r[0], r[1]), info.pages[0].user_to_view(r[2], r[3]));
        let (w, h) = ((a[0] - b[0]).abs(), (a[1] - b[1]).abs());
        assert!(w > 4.0 * h, "rotation {degrees}: the line's displayed box is {w} × {h}, not across");
    }
}

#[test]
fn text_marks_and_date() {
    let mut h = harness();
    assert!(h.execute("sign.fill.check"));
    assert!(matches!(h.state().quick_tool, QuickTool::Fill(_)));
    click(&mut h, 50.0, 350.0);
    click(&mut h, 80.0, 350.0);
    assert_eq!(h.state().quick_tool, QuickTool::Fill(FillTool::Check), "checkmarks stay repeatable");
    h.execute("sign.fill.date");
    click(&mut h, 50.0, 300.0);
    h.execute("sign.fill.text");
    click(&mut h, 50.0, 250.0);
    h.event(egui::Event::Text("Ada Lovelace".into()));
    h.run_steps(1);
    commit_typed_text(&mut h);
    h.run_steps(4);
    let v = items(&h);
    assert_eq!(v.len(), 4, "{v:?}");
    assert!(v.iter().any(|(t, c)| t == "FreeText" && c.as_deref() == Some("Ada Lovelace")));
    assert!(v.iter().any(|(t, c)| t == "FreeText" && c.as_deref().is_some_and(|c| c.matches('/').count() == 2)), "a date: {v:?}");
    assert!(v.iter().any(|(t, _)| t == "Stamp"));
}

/// The open type box, found by its accessible name for either input role.
fn type_box_rect(h: &Harness<'static, PdfCraftApp>) -> egui::Rect {
    h.get_by_label("Text to place").rect()
}

#[test]
fn clicking_elsewhere_with_add_text_keeps_the_typed_text() {
    let mut h = harness();
    h.execute("sign.fill.text");
    click(&mut h, 50.0, 250.0);
    h.event(egui::Event::Text("\nAda\nLovelace\n".into()));
    h.run_steps(1);
    // Still on Add text: the next click starts a new box, and must keep the first one.
    click(&mut h, 50.0, 150.0);
    h.run_steps(2);
    let v = items(&h);
    assert_eq!(v, vec![("FreeText".to_string(), Some("\nAda\nLovelace\n".to_string()))]);
    assert!(h.state().views[0].session.fill_text.as_ref().is_some_and(|t| t.text.is_empty()), "a new, empty box is open");
}

/// Escape steps back out of a Fill & Sign tool as out of a comment tool: the first press closes
/// the open type box, the next one leaves the Text tool for Select, so a click opens no new box.
#[test]
fn escape_leaves_the_fill_and_sign_tools() {
    let mut h = harness();
    h.execute("sign.fill.text");
    click(&mut h, 50.0, 250.0);
    assert!(h.state().views[0].session.fill_text.is_some(), "a box is open");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(h.state().views[0].session.fill_text.is_none(), "the first Escape closes the box");
    assert_eq!(h.state().quick_tool, QuickTool::Fill(FillTool::Text), "the tool stays for the next box");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert_eq!(h.state().quick_tool, QuickTool::Select, "the next Escape leaves the tool");
    click(&mut h, 50.0, 150.0);
    assert!(h.state().views[0].session.fill_text.is_none(), "no new box");
    assert!(items(&h).is_empty(), "nothing was added: {:?}", items(&h));
    // A mark tool too.
    h.execute("sign.fill.check");
    h.run_steps(2);
    assert_eq!(h.state().quick_tool, QuickTool::Fill(FillTool::Check));
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert_eq!(h.state().quick_tool, QuickTool::Select);
}

/// A box opened while a comment is selected deselects it: two presses of Escape still close the
/// box and leave the tool.
#[test]
fn escape_leaves_the_text_tool_in_two_presses_after_a_selection() {
    let mut h = harness();
    h.execute("sign.fill.text");
    click(&mut h, 50.0, 250.0);
    h.event(egui::Event::Text("Ada".into()));
    h.run_steps(1);
    commit_typed_text(&mut h);
    assert_eq!(items(&h).len(), 1, "{:?}", items(&h));
    h.state_mut().views[0].session.fill_text = None;
    h.state_mut().views[0].session.comments.select_only((0, 0));
    click(&mut h, 50.0, 150.0);
    assert!(h.state().views[0].session.fill_text.is_some(), "a box is open");
    assert_eq!(h.state().views[0].session.comments.primary(), None, "the new box deselected the comment");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(h.state().views[0].session.fill_text.is_none(), "the first Escape closes the box");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert_eq!(h.state().quick_tool, QuickTool::Select, "the second Escape leaves the tool");
}

#[test]
fn fill_text_size_controls_new_text_and_survives_settings_restore() {
    let mut h = harness();
    h.execute("sign.fill.text");
    h.run_steps(2);
    h.get_by_label("Font size");
    h.state_mut().fill_text_size = 24.0;
    click(&mut h, 50.0, 250.0);
    assert_eq!(h.state().views[0].session.fill_text.as_ref().map(|t| t.size), Some(24.0));
    h.event(egui::Event::Text("Larger text".into()));
    h.run_steps(1);
    commit_typed_text(&mut h);
    h.run_steps(3);
    let annotation =
        h.state().session.get(h.state().views[0].id).unwrap().info.annotations.iter().find(|a| a.contents.as_deref() == Some("Larger text")).unwrap();
    assert!(annotation.rect[3] - annotation.rect[1] > 24.0, "24 pt text has a larger annotation: {annotation:?}");
    let mut restored = crate::control::new_app();
    restore_from(&mut restored, h.state());
    assert_eq!(restored.fill_text_size, 24.0);
    restore_json(&mut restored, r#"{"fill_text_size":1000000}"#);
    assert_eq!(restored.fill_text_size, 24.0, "invalid settings do not change the size");
}

/// #1274: with a placed Fill & Sign text selected, the Font size control resizes that text,
/// not only text typed afterwards; with nothing selected it changes nothing on the page.
#[test]
fn fill_text_size_resizes_the_selected_text() {
    let mut h = harness();
    h.execute("sign.fill.text");
    click(&mut h, 50.0, 250.0);
    h.event(egui::Event::Text("Resize me".into()));
    h.run_steps(1);
    commit_typed_text(&mut h);
    h.run_steps(3);
    let height = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        let a = s.session.get(s.views[0].id).unwrap().info.annotations.iter().find(|a| a.contents.as_deref() == Some("Resize me")).cloned().unwrap();
        a.rect[3] - a.rect[1]
    };
    let before = height(&h);
    let set_size = |h: &mut Harness<'static, PdfCraftApp>, value: &str| {
        h.get_by_role_and_label(egui::accesskit::Role::Button, "Font size").click();
        h.run_steps(2);
        h.get_by_role_and_label(egui::accesskit::Role::SpinButton, "Font size").click();
        h.run_steps(2);
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        h.event(egui::Event::Text(value.into()));
        h.run_steps(1);
        h.key_press(egui::Key::Enter);
        h.run_steps(4);
        h.get_by_role_and_label(egui::accesskit::Role::Button, "Font size").click();
        h.run_steps(2);
    };
    // Nothing selected: only the size for new text changes.
    h.state_mut().views[0].session.comments.clear_selection();
    set_size(&mut h, "18");
    assert_eq!(h.state().fill_text_size, 18.0);
    assert_eq!(height(&h), before, "an unselected placed text keeps its size");
    // The placed text picked up with the Add text tool, then resized.
    let annotation = {
        let s = h.state();
        s.session
            .get(s.views[0].id)
            .unwrap()
            .info
            .annotations
            .iter()
            .find(|a| a.contents.as_deref() == Some("Resize me"))
            .map(|a| (a.page, a.index))
            .unwrap()
    };
    h.state_mut().views[0].session.comments.select_only(annotation);
    set_size(&mut h, "36");
    assert_eq!(h.state().fill_text_size, 36.0);
    let after = height(&h);
    assert!(after > before * 2.0, "36 pt text has a taller box than 10 pt text: {before} -> {after}");
}

/// Dragging the Font size control with a placed text selected resizes it once, when the drag
/// ends, to the size it was dragged to: the frame of the release must not put the old size back.
#[test]
fn dragging_the_font_size_resizes_the_selected_text_to_where_the_drag_ends() {
    let mut h = harness();
    h.execute("sign.fill.text");
    click(&mut h, 50.0, 250.0);
    h.event(egui::Event::Text("Drag me".into()));
    h.run_steps(1);
    commit_typed_text(&mut h);
    let (id, annotation) = {
        let s = h.state();
        let id = s.views[0].id;
        let a = s.session.get(id).unwrap().info.annotations.iter().find(|a| a.contents.as_deref() == Some("Drag me")).map(|a| (a.page, a.index));
        (id, a.unwrap())
    };
    h.state_mut().views[0].session.comments.select_only(annotation);
    h.run_steps(2);
    // The size is set in the Font size button's popup.
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Font size").click();
    h.run_steps(2);
    let from = h.get_by_role_and_label(egui::accesskit::Role::Slider, "Font size").rect().center();
    let before = h.state().fill_text_size;
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    for dx in [10.0, 20.0, 30.0, 40.0] {
        h.hover_at(from + egui::vec2(dx, 0.0));
        h.run_steps(1);
    }
    h.drop_at(from + egui::vec2(40.0, 0.0));
    h.run_steps(4);
    let dragged = h.state().fill_text_size;
    assert!(dragged > before, "the drag made the size larger: {before} -> {dragged}");
    let placed = h.state().session.get(id).unwrap().comment_props(annotation.0, annotation.1).and_then(|p| p.font_size);
    assert_eq!(placed.map(|size| format!("{size:.3}")), Some(format!("{dragged:.3}")), "the selected text has the size the drag ended at");
}

/// The font sizes of the texts `text` drawn in the last frame.
fn drawn_sizes(h: &Harness<'static, PdfCraftApp>, text: &str) -> Vec<f32> {
    fn walk(shape: &egui::Shape, text: &str, out: &mut Vec<f32>) {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, text, out)),
            egui::Shape::Text(t) if t.galley.text() == text => {
                out.extend(t.galley.job.sections.first().map(|s| s.format.font_id.size));
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in &h.output().shapes {
        walk(&clipped.shape, text, &mut out);
    }
    out
}

/// The type box draws its text at the page's zoom whichever way the view is turned. With the
/// view turned a quarter, the page's width runs down the screen: reading the zoom off the screen
/// rectangle's width instead made the typed text a height-over-width too large (here 4/3).
#[test]
fn the_type_box_text_matches_the_page_zoom_in_a_turned_view() {
    for rotate in ["0", "90"] {
        let mut h = harness();
        h.set_option("rotate", rotate).unwrap();
        h.run_steps(3);
        h.execute("sign.fill.text");
        click(&mut h, 50.0, 250.0);
        h.event(egui::Event::Text("Ada".into()));
        h.run_steps(2);
        let s = h.state();
        let page = &s.session.get(s.views[0].id).unwrap().info.pages[0];
        let rect = s.views[0].page_screen_rect(0).expect("on screen");
        // Screen pixels per page point: the page's width lies across, or down when turned.
        let zoom = if rotate == "90" { rect.height() } else { rect.width() } / page.width;
        let size = s.views[0].session.fill_text.as_ref().expect("an open type box").size as f32;
        let drawn = drawn_sizes(&h, "Ada");
        assert!(!drawn.is_empty(), "{rotate}°: the typed text is drawn");
        for d in drawn {
            assert!((d - size * zoom).abs() < 0.5, "{rotate}°: drawn at {d}, the page shows it at {}", size * zoom);
        }
    }
}

/// With a placed text selected, the toolbar shows its own border, also after Comment Properties
/// changed it, and changes only that text: the Border colour button gives its border its own
/// colour ("Same as text" takes it back), and new text keeps the toolbar's own border.
#[test]
fn the_toolbar_shows_and_changes_the_selected_texts_border() {
    let mut h = harness();
    h.execute("sign.fill.text");
    click(&mut h, 50.0, 250.0);
    h.event(egui::Event::Text("Framed".into()));
    h.run_steps(1);
    commit_typed_text(&mut h);
    let (page, index) = {
        let s = h.state();
        let a = s.session.get(s.views[0].id).unwrap().info.annotations.iter().find(|a| a.contents.as_deref() == Some("Framed")).cloned().unwrap();
        (a.page, a.index)
    };
    let props = |h: &Harness<'static, PdfCraftApp>| h.state().session.get(h.state().views[0].id).unwrap().comment_props(page, index).unwrap();
    let shows = |h: &Harness<'static, PdfCraftApp>, value: &str| {
        h.query_all_by_role(egui::accesskit::Role::ComboBox).any(|n| n.value().as_deref() == Some(value))
    };
    h.state_mut().views[0].session.comments.select_only((page, index));
    // As Comment Properties ▸ Thickness does.
    let ctx_h = h.ctx.clone();
    h.state_mut()
        .apply_edit(&ctx_h, pdfcraft_engine::Edit::StyleAnnotation { page, index, color: None, opacity: None, width: Some(2.0), endings: None });
    h.run_steps(3);
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Border").click();
    h.run_steps(2);
    assert!(shows(&h, "2 pt"), "the toolbar shows the selected text's border");
    assert_eq!(h.state().fill_text_border, 0.0, "new text keeps none");

    h.get_by_label("Border colour").click();
    h.run_steps(2);
    h.get_by_label("Red").click();
    h.run_steps(4);
    let red = Some([0.89, 0.13, 0.13]);
    assert_eq!(props(&h).border_color, red, "the selected text's border is red");
    assert_eq!(h.state().fill_text_border_color, None, "new text's border keeps the text's colour");
    h.get_by_label("Border colour").click();
    h.run_steps(2);
    h.get_by_label("Same as text").click();
    h.run_steps(4);
    assert_eq!(props(&h).border_color, None);
    // Nothing selected, the toolbar shows new text's border again.
    h.state_mut().views[0].session.comments.clear_selection();
    h.run_steps(2);
    assert!(shows(&h, "None"));
}

/// With a border, typed text can have an oval or a circle around it instead of a box (a
/// position number in a circle on a plan): the toolbar's shape control sets it for new text and
/// for the selected text, and shows the selected text's own. The text stays where it was typed.
#[test]
fn typed_text_takes_an_oval_or_a_circle_from_the_toolbar() {
    use pdfcraft_engine::BorderShape;
    let mut h = harness();
    h.execute("sign.fill.text");
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Border").click();
    h.run_steps(2);
    assert_eq!(h.query_all_by_label("Border shape").count(), 0, "no shape without a border");
    h.state_mut().fill_text_border = 1.0;
    h.run_steps(2);
    // The shape control shows "Rectangle"; pick Circle for new text. It is the last of the text's
    // controls, so in a vertical bar it may sit below the bar's end: scrolled to first, as by hand.
    h.query_all_by_role(egui::accesskit::Role::ComboBox)
        .find(|n| n.value().as_deref() == Some("Rectangle"))
        .expect("the shape control")
        .scroll_to_me();
    h.run_steps(2);
    h.query_all_by_role(egui::accesskit::Role::ComboBox).find(|n| n.value().as_deref() == Some("Rectangle")).expect("the shape control").click();
    h.run_steps(2);
    h.get_by_label("Circle").click();
    h.run_steps(2);
    assert_eq!(h.state().fill_text_border_shape, BorderShape::Circle);
    click(&mut h, 50.0, 250.0);
    h.event(egui::Event::Text("12".into()));
    h.run_steps(1);
    commit_typed_text(&mut h);
    let (page, index) = {
        let s = h.state();
        let a = s.session.get(s.views[0].id).unwrap().info.annotations.iter().find(|a| a.contents.as_deref() == Some("12")).cloned().unwrap();
        (a.page, a.index)
    };
    let props = |h: &Harness<'static, PdfCraftApp>| h.state().session.get(h.state().views[0].id).unwrap().comment_props(page, index).unwrap();
    assert_eq!(props(&h).border_shape, Some(BorderShape::Circle), "new text gets the circle");
    let r = h.state().session.get(h.state().views[0].id).unwrap().info.annotations.iter().find(|a| a.index == index).unwrap().rect;
    assert!(((r[2] - r[0]) - (r[3] - r[1])).abs() < 0.01, "a circle's square box: {r:?}");
    // Snug around "12" at 10 pt: its box no wider than the text needs (not the 40 pt a box to type
    // into keeps), so the circle is as wide as that box, where it was 50 pt around the wider one.
    let w = pdfcraft_engine::annot_text::text_width("12", 10.0) + 10.0;
    let across = f64::from(r[2] - r[0]);
    assert!((across - w.max(20.0)).abs() < 0.01, "the circle is {across} pt across, not {w}: {r:?}");
    // Selected, an oval from the toolbar: one undo step.
    h.state_mut().views[0].session.comments.select_only((page, index));
    h.run_steps(3);
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Border").click();
    h.run_steps(2);
    h.query_all_by_role(egui::accesskit::Role::ComboBox)
        .find(|n| n.value().as_deref() == Some("Circle"))
        .expect("shows the text's own")
        .scroll_to_me();
    h.run_steps(2);
    h.query_all_by_role(egui::accesskit::Role::ComboBox).find(|n| n.value().as_deref() == Some("Circle")).expect("shows the text's own").click();
    h.run_steps(2);
    h.get_by_label("Oval").click();
    h.run_steps(4);
    assert_eq!(props(&h).border_shape, Some(BorderShape::Oval));
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Change comment properties"));
    assert_eq!(h.state().fill_text_border_shape, BorderShape::Circle, "new text keeps the circle");
    // As Comment Properties does: the toolbar shows it.
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(&ctx_h, pdfcraft_engine::Edit::BorderShapeAnnotation { page, index, shape: BorderShape::Rectangle });
    h.run_steps(3);
    assert!(h.query_all_by_role(egui::accesskit::Role::ComboBox).any(|n| n.value().as_deref() == Some("Rectangle")), "the selected text's own");

    // The circle for new text lasts until PdfCraft quits; a saved default is read back.
    let mut restored = crate::control::new_app();
    restore_from(&mut restored, h.state());
    assert_eq!(restored.fill_text_border_shape, BorderShape::Rectangle);
    restore_json(&mut restored, r#"{"fill_text_border_shape":"oval"}"#);
    assert_eq!(restored.fill_text_border_shape, BorderShape::Oval);
    restore_json(&mut restored, r#"{"fill_text_border_shape":"star"}"#);
    assert_eq!(restored.fill_text_border_shape, BorderShape::Oval, "an unknown shape keeps the setting");
}

/// Border ▸ None takes an oval or a circle away with its line, from the selected text and for new
/// text: without one, the text's box stayed grown around a shape that isn't drawn.
#[test]
fn no_border_takes_the_oval_or_circle_away_too() {
    use pdfcraft_engine::BorderShape;
    let mut h = harness();
    h.execute("sign.fill.text");
    h.state_mut().fill_text_border = 1.0;
    h.state_mut().fill_text_border_shape = BorderShape::Circle;
    h.run_steps(2);
    click(&mut h, 50.0, 250.0);
    h.event(egui::Event::Text("12".into()));
    h.run_steps(1);
    commit_typed_text(&mut h);
    let (page, index) = {
        let s = h.state();
        let a = s.session.get(s.views[0].id).unwrap().info.annotations.iter().find(|a| a.contents.as_deref() == Some("12")).cloned().unwrap();
        (a.page, a.index)
    };
    let props = |h: &Harness<'static, PdfCraftApp>| h.state().session.get(h.state().views[0].id).unwrap().comment_props(page, index).unwrap();
    assert_eq!(props(&h).border_shape, Some(BorderShape::Circle));
    let no_border = |h: &mut Harness<'static, PdfCraftApp>| {
        if h.query_all_by_role_and_label(egui::accesskit::Role::Dialog, "Border").count() == 0 {
            h.get_by_role_and_label(egui::accesskit::Role::Button, "Border").click();
            h.run_steps(2);
        }
        h.query_all_by_role(egui::accesskit::Role::ComboBox).find(|n| n.value().as_deref() == Some("1 pt")).expect("the Border control").click();
        h.run_steps(2);
        h.get_by_label("None").click();
        h.run_steps(4);
    };
    // The selected text: no line, and a rectangle again, its box the text's.
    h.state_mut().views[0].session.comments.select_only((page, index));
    h.run_steps(3);
    no_border(&mut h);
    assert_eq!(props(&h).width, Some(0.0));
    assert_eq!(props(&h).border_shape.unwrap_or_default(), BorderShape::Rectangle);
    // One undo step brings back the line and the circle together.
    undo(&mut h);
    h.run_steps(2);
    assert_eq!((props(&h).width, props(&h).border_shape), (Some(1.0), Some(BorderShape::Circle)), "one undo step");
    redo(&mut h);
    h.run_steps(2);
    assert_eq!(h.state().fill_text_border_shape, BorderShape::Circle, "new text keeps its circle");
    // New text: none, and a rectangle when a border is chosen again.
    h.state_mut().views[0].session.comments.clear_selection();
    h.run_steps(3);
    no_border(&mut h);
    assert_eq!((h.state().fill_text_border, h.state().fill_text_border_shape), (0.0, BorderShape::Rectangle));
}

/// The Border control puts a frame around new Fill & Sign text, and around the text that is
/// selected; the box grows outwards, so the text stays where it was typed. The choice persists.
#[test]
fn fill_text_border_frames_new_and_selected_text() {
    let mut h = harness();
    h.execute("sign.fill.text");
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Border").click();
    h.run_steps(2);
    h.get_by_label("Border:");
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Border").click();
    h.run_steps(2);
    let type_at = |h: &mut Harness<'static, PdfCraftApp>, y: f32| {
        click(h, 50.0, y);
        h.event(egui::Event::Text("Same".into()));
        h.run_steps(1);
        commit_typed_text(h);
    };
    // The two texts, oldest first: (page, index, rect).
    let texts = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let mut v: Vec<_> =
            doc.info.annotations.iter().filter(|a| a.contents.as_deref() == Some("Same")).map(|a| (a.page, a.index, a.rect)).collect();
        v.sort_by_key(|t| t.1);
        v
    };
    let width = |r: [f32; 4]| r[2] - r[0];
    type_at(&mut h, 150.0);
    // Nothing selected, so the toolbar sets new text's border only (it shows a selected text's).
    h.state_mut().views[0].session.comments.clear_selection();
    h.state_mut().fill_text_border = 1.5;
    type_at(&mut h, 250.0);
    let [(page, index, plain), (_, _, framed)] = texts(&h)[..] else { panic!("two texts") };
    assert!((width(framed) - width(plain) - 3.0).abs() < 0.01, "1.5 pt on each side: {plain:?} {framed:?}");

    // The plain text selected, then framed with 2 pt from the toolbar.
    h.state_mut().views[0].session.comments.select_only((page, index));
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Border").click();
    h.run_steps(2);
    h.query_all_by_role(egui::accesskit::Role::ComboBox)
        .find(|n| n.value().as_deref() == Some("None"))
        .expect("the Border control, showing the selected text's border")
        .click();
    h.run_steps(2);
    h.get_by_label("2 pt").click();
    h.run_steps(4);
    assert_eq!(h.state().fill_text_border, 1.5, "new text keeps its own");
    let (_, _, now) = texts(&h)[0];
    assert!((width(now) - width(plain) - 4.0).abs() < 0.01, "2 pt on each side: {plain:?} {now:?}");
    assert!((plain[0] - now[0] - 2.0).abs() < 0.01 && (now[3] - plain[3] - 2.0).abs() < 0.01, "grown outwards, the text in place: {plain:?} {now:?}");

    // The toolbar's border lasts until PdfCraft quits; a saved default is read back.
    let mut restored = crate::control::new_app();
    restore_from(&mut restored, h.state());
    assert_eq!(restored.fill_text_border, 0.0);
    restore_json(&mut restored, r#"{"fill_text_border":2}"#);
    assert_eq!(restored.fill_text_border, 2.0);
    restore_json(&mut restored, r#"{"fill_text_border":100}"#);
    assert_eq!(restored.fill_text_border, 2.0, "invalid settings do not change the border");
}

/// Make Properties Default on typed text sets what Fill & Sign gives new text, its size and
/// border, and keeps it across launches.
#[test]
fn typed_text_made_default_is_what_new_text_gets() {
    use pdfcraft_engine::{BorderShape, Edit};
    let mut h = harness();
    h.execute("sign.fill.text");
    let type_at = |h: &mut Harness<'static, PdfCraftApp>, y: f32, text: &str| {
        click(h, 50.0, y);
        h.event(egui::Event::Text(text.into()));
        h.run_steps(1);
        commit_typed_text(h);
        let s = h.state();
        let a = s.session.get(s.views[0].id).unwrap().info.annotations.iter().find(|a| a.contents.as_deref() == Some(text)).cloned().unwrap();
        (a.page, a.index)
    };
    let (page, index) = type_at(&mut h, 150.0, "Model");
    let red = Some([0.89, 0.13, 0.13]);
    for edit in [
        Edit::StyleAnnotation { page, index, color: None, opacity: None, width: Some(1.5), endings: None },
        Edit::BorderColorAnnotation { page, index, color: red },
        Edit::BorderShapeAnnotation { page, index, shape: BorderShape::Circle },
        Edit::FontSizeAnnotation { page, index, size: 14.0 },
    ] {
        let ctx_h = h.ctx.clone();
        assert!(h.state_mut().apply_edit(&ctx_h, edit));
    }
    let ctx_h = h.ctx.clone();
    h.state_mut().open_comment_props(&ctx_h, page, index);
    h.run_steps(2);
    h.get_by_label("Make Properties Default").click();
    h.run_steps(1);
    h.get_by_label("OK").click();
    h.run_steps(3);
    let s = h.state();
    assert_eq!((s.fill_text_size, s.fill_text_border, s.fill_text_border_color, s.fill_text_border_shape), (14.0, 1.5, red, BorderShape::Circle));
    let mut restored = crate::control::new_app();
    restore_from(&mut restored, h.state());
    assert_eq!(
        (restored.fill_text_size, restored.fill_text_border, restored.fill_text_border_color, restored.fill_text_border_shape),
        (14.0, 1.5, red, BorderShape::Circle),
        "kept across launches"
    );
    h.state_mut().views[0].session.comments.clear_selection();
    let (page, index) = type_at(&mut h, 350.0, "Next");
    let p = h.state().session.get(h.state().views[0].id).unwrap().comment_props(page, index).unwrap();
    assert_eq!((p.font_size, p.width, p.border_color, p.border_shape), (Some(14.0), Some(1.5), red, Some(BorderShape::Circle)));
}

#[test]
fn fill_text_colour_survives_preview_commit_settings_and_save() {
    let mut h = harness();
    h.execute("sign.fill.text");
    h.run_steps(2);
    h.get_by_label("Colour").click();
    h.run_steps(2);
    h.get_by_label("Blue").click();
    h.run_steps(2);
    let color = h.state().fill_text_color;
    assert!(color[2] > color[0], "blue was selected: {color:?}");
    click(&mut h, 50.0, 250.0);
    assert_eq!(h.state().views[0].session.fill_text.as_ref().unwrap().color, color, "the editor uses the selected colour");
    h.event(egui::Event::Text("Blue form text".into()));
    h.run_steps(2);
    commit_typed_text(&mut h);
    h.run_steps(3);
    let id = h.state().views[0].id;
    let expected = color.map(|v| v as f32);
    let annotation = h.state().session.get(id).unwrap().info.annotations.iter().find(|a| a.contents.as_deref() == Some("Blue form text")).unwrap();
    assert_eq!(annotation.color, Some(expected));
    let bytes = h.state().session.save_bytes(id).unwrap();
    let reopened = harness_bytes(&bytes);
    let saved = reopened
        .state()
        .session
        .get(reopened.state().views[0].id)
        .unwrap()
        .info
        .annotations
        .iter()
        .find(|a| a.contents.as_deref() == Some("Blue form text"))
        .unwrap();
    assert_eq!(saved.color, Some(expected));
    let mut restored = crate::control::new_app();
    restore_from(&mut restored, h.state());
    assert_eq!(restored.fill_text_color, color);
    restore_json(&mut restored, r#"{"fill_text_color":[2,0,0]}"#);
    assert_eq!(restored.fill_text_color, color, "invalid saved colours are ignored");
    h.execute("edit.undo");
    h.run_steps(2);
    assert!(!items(&h).iter().any(|(_, text)| text.as_deref() == Some("Blue form text")));
    h.execute("edit.redo");
    h.run_steps(2);
    assert!(items(&h).iter().any(|(_, text)| text.as_deref() == Some("Blue form text")));
}

#[test]
fn the_type_box_grows_as_text_is_typed() {
    let mut h = harness();
    h.execute("sign.fill.text");
    click(&mut h, 50.0, 250.0);
    h.run_steps(2);
    let empty = type_box_rect(&h).width();
    h.event(egui::Event::Text("Ada Lovelace, Countess of Lovelace".into()));
    h.run_steps(3);
    let full = type_box_rect(&h).width();
    assert!(full > empty * 2.0, "the box widens with its text: {empty} -> {full}");
}

#[test]
fn signing_draws_a_signature_once_and_places_it() {
    let mut h = harness();
    assert!(h.execute("sign.fill.signature"));
    h.run_steps(2);
    assert!(pad_open(&h), "no signature yet: the pad opens");
    assert!(shows(&h, "Create signature"));
    h.get_all_by_label("Draw").last().unwrap().click();
    h.run_steps(2);
    let pad = h.get_by_role_and_label(egui::accesskit::Role::Image, "Draw").rect();
    let start = pos2(pad.left() + pad.width() * 0.1, pad.center().y);
    let step = pad.width() * 0.8 / 8.0;
    h.hover_at(start);
    h.run_steps(1);
    h.drag_at(start);
    h.run_steps(1);
    for k in 1..=8 {
        h.hover_at(start + egui::vec2(k as f32 * step, if k % 2 == 0 { -20.0 } else { 20.0 }));
        h.run_steps(1);
    }
    h.drop_at(start + egui::vec2(8.0 * step, 0.0));
    h.run_steps(2);
    h.get_by_label("Apply").click();
    h.run_steps(3);
    assert!(h.state().signature.is_some());
    assert!(!pad_open(&h));
    click(&mut h, 60.0, 100.0);
    assert!(items(&h).iter().any(|(t, _)| t == "Ink"));
    assert_eq!(h.state().quick_tool, QuickTool::Select);
    assert_eq!(h.state().views[0].session.comments.primary(), Some((0, 0)));
    // The signature is remembered (persisted with the app's settings).
    let mut again = crate::control::new_app();
    again.set_option(&egui::Context::default(), "language", "en").unwrap();
    restore_from(&mut again, h.state());
    assert!(again.signature.is_some());
}

#[test]
fn typed_signatures_and_initials() {
    let mut h = harness();
    h.state_mut().comment_prefs.author = "Grace Hopper".into();
    assert!(h.execute("sign.fill.signature"));
    h.run_steps(2);
    // Type is the default, with the author's name filled in: the typed pad is named by its text.
    assert!(shows(&h, "Create signature"));
    assert!(shows(&h, "Grace Hopper"), "the pad shows the author's name");
    h.get_by_label("Apply").click();
    h.run_steps(3);
    assert_eq!(h.state().signature, Some(SavedSig::Typed("Grace Hopper".into())));
    click(&mut h, 60.0, 100.0);
    assert!(items(&h).iter().any(|(t, _)| t == "Stamp"), "typed signatures are filled outlines");
    assert_eq!(h.state().quick_tool, QuickTool::Select);
    // Initials: their own pad (GH), then placed.
    assert!(h.execute("sign.fill.initials"));
    h.run_steps(2);
    assert!(shows(&h, "Create initials"));
    assert!(shows(&h, "GH"), "the pad shows the author's initials");
    h.get_by_label("Apply").click();
    h.run_steps(3);
    click(&mut h, 60.0, 160.0);
    assert_eq!(items(&h).iter().filter(|(t, _)| t == "Stamp").count(), 2);
    // Both are remembered.
    let mut again = crate::control::new_app();
    again.set_option(&egui::Context::default(), "language", "en").unwrap();
    restore_from(&mut again, h.state());
    assert_eq!(again.signature, h.state().signature);
    assert_eq!(again.initials, Some(SavedSig::Typed("GH".into())));
}

#[test]
fn changing_saved_signatures_and_initials_preserves_placed_marks() {
    let mut h = harness();
    h.state_mut().signature = Some(SavedSig::Typed("Ada Lovelace".into()));
    h.state_mut().initials = Some(SavedSig::Typed("AL".into()));
    h.execute("sign.fill.signature");
    h.run_steps(2);
    assert!(!pad_open(&h), "Sign still reuses the saved signature");
    click(&mut h, 40.0, 300.0);
    let original = format!("{:?}", h.state().session.get(h.state().views[0].id).unwrap().info.annotations);

    // Replacing the saved signature leaves the placement tool and the placed marks alone.
    assert!(h.execute("sign.fill.signature.change"));
    h.run_steps(2);
    assert!(pad_open(&h));
    assert!(shows(&h, "Change signature"));
    assert!(shows(&h, "Ada Lovelace"), "the pad starts from the saved signature");
    h.get_by_label("Type text").click();
    h.run_steps(1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.event(egui::Event::Text("Grace Hopper".into()));
    h.run_steps(2);
    h.get_by_label("Apply").click();
    h.run_steps(3);
    assert_eq!(h.state().signature, Some(SavedSig::Typed("Grace Hopper".into())));
    assert_eq!(h.state().initials, Some(SavedSig::Typed("AL".into())));
    assert_eq!(h.state().quick_tool, QuickTool::Fill(FillTool::Signature));
    assert_eq!(format!("{:?}", h.state().session.get(h.state().views[0].id).unwrap().info.annotations), original);
    click(&mut h, 40.0, 200.0);
    assert_eq!(items(&h).len(), 2);

    assert!(h.execute("sign.fill.initials.change"));
    h.run_steps(2);
    assert!(shows(&h, "Change initials"));
    assert!(shows(&h, "AL"), "the pad starts from the saved initials");
    h.get_by_label("Type text").click();
    h.run_steps(1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.event(egui::Event::Text("GH".into()));
    h.run_steps(2);
    h.get_by_label("Apply").click();
    h.run_steps(3);
    assert_eq!(h.state().quick_tool, QuickTool::Fill(FillTool::Initials));
    click(&mut h, 40.0, 100.0);
    assert_eq!(items(&h).len(), 3);
    let mut again = crate::control::new_app();
    again.set_option(&egui::Context::default(), "language", "en").unwrap();
    restore_from(&mut again, h.state());
    assert_eq!(again.signature, Some(SavedSig::Typed("Grace Hopper".into())));
    assert_eq!(again.initials, Some(SavedSig::Typed("GH".into())));
}

#[test]
fn saved_signature_cards_remove_and_add_without_changing_the_document() {
    let mut h = harness();
    h.state_mut().signature = Some(SavedSig::Typed("Ada Lovelace".into()));
    h.state_mut().initials = Some(SavedSig::Typed("AL".into()));
    h.execute("sign.fill.signature");
    click(&mut h, 40.0, 300.0);
    let original = format!("{:?}", h.state().session.get(h.state().views[0].id).unwrap().info.annotations);
    h.state_mut().left = pdfcraft_ui_egui::LeftPanel::Tool("fill_sign");
    h.state_mut().set_left_open(true);
    h.run_steps(3);
    h.get_by_label("Use signature").click();
    h.run_steps(2);
    assert!(!pad_open(&h), "the preview reuses the saved value");
    h.hover_at(h.get_by_label("Use signature").rect().center());
    h.run_steps(2);
    h.get_by_label("Remove saved signature").click();
    h.run_steps(3);
    assert_eq!(h.state().signature, None);
    assert_eq!(h.state().initials, Some(SavedSig::Typed("AL".into())));
    let mut again = crate::control::new_app();
    again.set_option(&egui::Context::default(), "language", "en").unwrap();
    restore_from(&mut again, h.state());
    assert_eq!(again.signature, None, "removal survives restart");
    h.get_by_label("Add signature").click();
    h.run_steps(3);
    h.get_by_label("Type text").click();
    h.run_steps(1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.event(egui::Event::Text("Grace Hopper".into()));
    h.run_steps(2);
    h.get_by_label("Apply").click();
    h.run_steps(3);
    assert_eq!(h.state().signature, Some(SavedSig::Typed("Grace Hopper".into())));
    h.hover_at(h.get_by_label("Use initials").rect().center());
    h.run_steps(2);
    h.get_by_label("Remove saved initials").click();
    h.run_steps(3);
    assert_eq!(h.state().initials, None);
    h.get_by_label("Add initials").click();
    h.run_steps(3);
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert_eq!(h.state().initials, None);
    assert_eq!(format!("{:?}", h.state().session.get(h.state().views[0].id).unwrap().info.annotations), original);
}

#[test]
fn long_typed_names_fit_the_placed_signature_and_keep_every_outline() {
    let text = "Alexandria Catherine Elizabeth Montgomery-Wellington";
    let sig = SavedSig::Typed(text.into());
    let page = pdfcraft_render::PageInfo { width: 300.0, height: 400.0, label: String::new(), crop: [0.0, 0.0, 300.0, 400.0], rotation: 0 };
    let pdfcraft_engine::Edit::AddAnnotation(a) = pdfcraft_ui_egui::fill_sign::place(0, &page, [40.0, 200.0], &sig, false, "").unwrap() else {
        panic!("expected annotation");
    };
    let pdfcraft_engine::Shape::TypedSignature { rect, contours } = a.shape else { panic!("expected typed signature") };
    assert!((rect[2] - rect[0] - 150.0).abs() < 0.001, "long names shrink to fit: {rect:?}");
    assert_eq!(contours.len(), pdfcraft_engine::script_outline(text).contours.len());
    assert!(contours.iter().flatten().all(|p| p.iter().all(|v| (0.0..=1.0).contains(v))), "all ink stays inside the appearance bounds");
    let mut h = harness();
    h.state_mut().signature = Some(sig.clone());
    h.execute("sign.fill.signature.change");
    h.run_steps(3);
    assert!(shows(&h, text), "the pad shows the whole name");
    h.get_by_label("Apply").click();
    h.run_steps(3);
    assert_eq!(h.state().signature, Some(sig));
    click(&mut h, 40.0, 200.0);
    let s = h.state();
    let a = &s.session.get(s.views[0].id).unwrap().info.annotations[0];
    assert!(a.rect[2] <= 190.001, "the full name fits on the page: {:?}", a.rect);
}

fn signature_file(test: &str) -> std::path::PathBuf {
    signature_file_with_mark(test, 10..20)
}

/// A 120 x 40 signature: a stroke across the middle and a block above it at the columns `mark`
/// (the rest is transparent), so a turned or flipped picture shows.
fn signature_file_with_mark(test: &str, mark: std::ops::Range<u32>) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("pdfcraft-signature-{test}-{}.png", std::process::id()));
    let image = image::RgbaImage::from_fn(120, 40, |x, y| {
        if (10..110).contains(&x) && (15..25).contains(&y) || mark.contains(&x) && (5..15).contains(&y) {
            image::Rgba([20, 30, 40, 255])
        } else {
            image::Rgba([0, 0, 0, 0])
        }
    });
    image.save(&path).unwrap();
    path
}

/// The picture in `path` as the saved-signature slot holds it.
fn saved_image(path: &std::path::Path) -> craft::review::SignatureImage {
    let image = pdfcraft_engine::SignatureImage::read(std::fs::File::open(path).unwrap()).unwrap();
    let [width, height] = image.size();
    craft::review::SignatureImage::new(width as u32, height as u32, image.rgba().to_vec())
}

/// Browse… on the pad's Image tab, answered with the file `path` as the user choosing it.
fn browse_image(h: &mut Harness<'static, PdfCraftApp>, files: &ScriptedFiles, path: &std::path::Path) {
    files.queue(Picked::Contents(vec![("signature".into(), std::fs::read(path).unwrap())]));
    h.get_by_label("Browse…").click();
    settle_decode(h);
}

/// The pad decodes a picked image on a worker: step frames until it has arrived.
fn settle_decode(h: &mut Harness<'static, PdfCraftApp>) {
    for _ in 0..40 {
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn image_signatures_and_initials_can_be_imported_placed_and_remembered() {
    let _gpu = gpu();
    let path = signature_file("import");
    let files = Arc::new(ScriptedFiles::new());
    let mut h = harness_with_files(&files);
    for (command, y, initials) in [("sign.fill.signature", 250.0, false), ("sign.fill.initials", 150.0, true)] {
        h.execute(command);
        h.run_steps(2);
        h.get_by_label("Image").click();
        h.run_steps(2);
        assert!(!can_apply(&h), "Apply needs an image");
        browse_image(&mut h, &files, &path);
        assert!(can_apply(&h), "the picked image is ready to apply");
        if !initials && let Ok(dir) = std::env::var("PDFCRAFT_SHOTS") {
            h.render().unwrap().save(format!("{dir}/image-signature-dialog.png")).unwrap();
        }
        assert_eq!(items(&h).len(), usize::from(initials), "importing doesn't edit the PDF");
        h.get_by_label("Apply").click();
        h.run_steps(3);
        click(&mut h, 40.0, y);
        assert_eq!(h.state().quick_tool, QuickTool::Select);
        assert_eq!(h.state().views[0].session.comments.primary(), Some((0, usize::from(initials))));
    }
    assert_eq!(items(&h).len(), 2);
    let state = h.state();
    let doc = state.session.get(state.views[0].id).unwrap();
    for a in &doc.info.annotations {
        assert_eq!(a.subtype, "Stamp");
        assert!(((a.rect[2] - a.rect[0]) / (a.rect[3] - a.rect[1]) - 3.0).abs() < 0.001);
        assert!(a.rect[2] - a.rect[0] <= 150.0);
    }
    assert_eq!(doc.can_undo(), Some("Add initials"));
    let settings = serde_json::to_string(&state.settings_snapshot()).unwrap();
    assert!(!settings.contains(&path.to_string_lossy().to_string()), "only the image is saved, never its source path");
    let mut again = crate::control::new_app();
    again.set_option(&egui::Context::default(), "language", "en").unwrap();
    restore_from(&mut again, state);
    assert_eq!(again.signature, state.signature);
    assert_eq!(again.initials, state.initials);
    let saved_image = state.signature.clone();
    h.execute("sign.fill.signature.change");
    h.run_steps(2);
    assert!(shows(&h, "Browse…"), "a saved image opens on the Image tab");
    assert!(can_apply(&h), "the saved image is in the pad");
    h.get_by_label("Clear").click();
    h.run_steps(2);
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert_eq!(h.state().signature, saved_image);
    h.execute("edit.undo");
    h.run_steps(3);
    assert_eq!(items(&h).len(), 1);
    h.execute("edit.redo");
    h.run_steps(3);
    assert_eq!(items(&h).len(), 2);
    // Show the saved preview cards in the actual shell as well as the dialog.
    h.set_option("tool", "fill_sign").unwrap();
    h.run_steps(3);
    h.get_by_label("Use signature");
    h.get_by_label("Use initials");
    if let Ok(dir) = std::env::var("PDFCRAFT_SHOTS") {
        h.render().unwrap().save(format!("{dir}/image-signatures.png")).unwrap();
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn image_signature_preview_follows_pointer_at_page_size_with_zoom_and_rotation() {
    let _gpu = gpu();
    // The block above the stroke is at its right end, away from the pointer's own cursor.
    let path = signature_file_with_mark("pointer", 100..110);
    let image = saved_image(&path);
    let mut h = harness();
    // A link under the placement point must not replace the image with a hand cursor.
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(
        &ctx_h,
        pdfcraft_engine::Edit::AddLink {
            page: 0,
            rect: [30.0, 230.0, 60.0, 270.0],
            action: pdfcraft_engine::LinkAction::Page { page: 0, zoom: pdfcraft_engine::LinkZoom::FitPage },
            style: pdfcraft_engine::LinkStyle::default(),
        },
    );
    h.state_mut().signature = Some(SavedSig::Image(image));
    h.execute("sign.fill.signature");
    // Exercise document rotation and view rotation separately and together. The image is upright
    // as displayed, whatever the page's /Rotate; the view rotation turns the whole screen.
    let mut previous_document_rotation = 0;
    for (document_rotation, view_rotation, zoom) in [(0, 0, "150"), (90, 0, "100"), (0, 90, "100"), (90, 90, "75"), (0, 180, "75"), (0, 270, "100")] {
        let delta = document_rotation - previous_document_rotation;
        if delta != 0 {
            let ctx_h = h.ctx.clone();
            assert!(h.state_mut().apply_edit(&ctx_h, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: delta }));
        }
        previous_document_rotation = document_rotation;
        while h.state().views[0].view.rotation != view_rotation {
            turn_view(&mut h);
        }
        h.set_option("zoom", zoom).unwrap();
        h.run_steps(3);
        for y in [250.0, 150.0] {
            h.hover_at(at(&h, 40.0, y));
            h.run_steps(2);
            assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::None, "the image replaces the crosshair");
            // The picture is 96 x 32 pt with its left edge at the pointer: the stroke spans 8..88
            // and the block above it 80..88. egui_kittest paints a mouse cursor, a 16 px triangle
            // down and right of the pointer, over the render, so only probe far from the pointer.
            let ink = shown(&h, 40.0, y, 50.0, 0.0);
            let margin = shown(&h, 40.0, y, 92.0, 0.0);
            let upper_stroke = shown(&h, 40.0, y, 84.0, 8.0);
            let lower_margin = shown(&h, 40.0, y, 84.0, -8.0);
            let pixels = h.render().unwrap();
            assert!(pixels.get_pixel(ink.x as u32, ink.y as u32).0[..3].iter().all(|v| *v < 60), "ink follows the pointer at the placed size");
            assert!(pixels.get_pixel(margin.x as u32, margin.y as u32).0[..3].iter().all(|v| *v > 240), "alpha exposes the page");
            assert!(
                pixels.get_pixel(upper_stroke.x as u32, upper_stroke.y as u32).0[..3].iter().all(|v| *v < 60),
                "the asymmetric image stays upright as displayed"
            );
            assert!(pixels.get_pixel(lower_margin.x as u32, lower_margin.y as u32).0[..3].iter().all(|v| *v > 240), "the image isn't flipped");
            if y == 150.0 {
                let old = shown(&h, 40.0, 250.0, 50.0, 0.0);
                assert!(pixels.get_pixel(old.x as u32, old.y as u32).0[..3].iter().all(|v| *v > 240), "moving the pointer removes the old preview");
            }
            if document_rotation == 0
                && view_rotation == 0
                && y == 250.0
                && let Ok(dir) = std::env::var("PDFCRAFT_SHOTS")
            {
                pixels.save(format!("{dir}/image-signature-pointer.png")).unwrap();
            }
        }
        assert!(items(&h).is_empty(), "hovering never adds a signature");
        assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().info.links.len(), 1, "the original link remains");
    }
    h.hover_at(pos2(5.0, 5.0));
    h.run_steps(2);
    assert_ne!(h.output().platform_output.cursor_icon, egui::CursorIcon::None, "restore the pointer off the page");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn image_signature_placement_selects_resize_handles_and_requires_reselecting_to_repeat() {
    let _gpu = gpu();
    let path = signature_file("resize");
    let image = saved_image(&path);
    let mut h = harness();
    h.state_mut().signature = Some(SavedSig::Image(image.clone()));
    h.state_mut().initials = Some(SavedSig::Image(image));
    h.state_mut().comment_prefs.pinned = true;
    h.execute("sign.fill.signature");
    click(&mut h, 40.0, 250.0);
    assert_eq!(h.state().quick_tool, QuickTool::Select, "signatures place once even when comment tools are pinned");
    assert_eq!(h.state().views[0].session.comments.primary(), Some((0, 0)));
    let placed = rendered_ink(&mut h, 90.0, 250.0);
    if let Ok(dir) = std::env::var("PDFCRAFT_SHOTS") {
        placed.save(format!("{dir}/image-signature-selected.png")).unwrap();
    }
    // The bottom-right handle is ready without another selection click.
    let corner = at(&h, 136.0, 234.0);
    h.hover_at(corner);
    h.run_steps(2);
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::ResizeNwSe);
    h.drag_at(corner);
    h.run_steps(1);
    let end = corner + egui::vec2(96.0, 32.0);
    h.hover_at(end);
    h.run_steps(2);
    h.drop_at(end);
    h.run_steps(3);
    let doc = h.state().session.get(h.state().views[0].id).unwrap();
    assert!(doc.info.annotations[0].rect.iter().zip([40.0, 218.0, 184.0, 266.0]).all(|(a, b)| (*a - b).abs() < 0.001));
    assert_eq!(doc.can_undo(), Some("Resize comment"));
    rendered_ink(&mut h, 160.0, 242.0);
    undo(&mut h);
    h.run_steps(2);
    assert!(
        h.state().session.get(h.state().views[0].id).unwrap().info.annotations[0]
            .rect
            .iter()
            .zip([40.0, 234.0, 136.0, 266.0])
            .all(|(a, b)| (*a - b).abs() < 0.001)
    );
    redo(&mut h);
    h.run_steps(2);
    click(&mut h, 40.0, 150.0);
    assert_eq!(items(&h).len(), 1, "a later page click doesn't add another signature");
    h.execute("sign.fill.signature");
    click(&mut h, 40.0, 150.0);
    assert_eq!(items(&h).len(), 2, "selecting the saved signature again places another");
    h.execute("sign.fill.initials");
    click(&mut h, 40.0, 100.0);
    assert_eq!(h.state().quick_tool, QuickTool::Select);
    assert_eq!(h.state().views[0].session.comments.primary(), Some((0, 2)));
    std::fs::remove_file(path).unwrap();
}

fn assert_rect(actual: [f32; 4], expected: [f32; 4]) {
    assert!(actual.iter().zip(expected).all(|(a, b)| (*a - b).abs() < 0.001), "{actual:?} != {expected:?}");
}

fn pixel_at(h: &Harness<'static, PdfCraftApp>, pixels: &image::RgbaImage, x: f32, y: f32) -> [u8; 4] {
    let p = at(h, x, y);
    pixels.get_pixel(p.x.round() as u32, p.y.round() as u32).0
}

fn has_blue_near(pixels: &image::RgbaImage, p: Pos2) -> bool {
    (-5..=5).any(|dy| {
        (-5..=5).any(|dx| {
            let v = pixels.get_pixel((p.x as i32 + dx) as u32, (p.y as i32 + dy) as u32).0;
            i32::from(v[2]) > i32::from(v[0]) + 40 && i32::from(v[2]) > i32::from(v[1]) + 35
        })
    })
}

#[test]
fn image_signature_live_resize_and_move_preserve_background_and_hide_moving_handles() {
    let _gpu = gpu();
    // Original synthetic page content; a white patch over the old image would fail this test.
    const GREEN_PAGE: &[u8] = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /Contents 4 0 R >> endobj\n4 0 obj << /Length 36 >> stream\n0.8 1 0.7 rg 0 0 300 400 re f\nendstream\nendobj\ntrailer << /Root 1 0 R >>\n%%EOF";
    let mut h = harness_bytes(GREEN_PAGE);
    let path = signature_file("live");
    let image = saved_image(&path);
    h.state_mut().signature = Some(SavedSig::Image(image));
    h.execute("sign.fill.signature");
    click(&mut h, 40.0, 250.0);
    rendered_ink(&mut h, 90.0, 250.0);
    let generation = h.state().session.get(h.state().views[0].id).unwrap().edit_generation();
    let corner = at(&h, 136.0, 234.0);
    h.drag_at(corner);
    h.run_steps(1);
    let end = at(&h, 184.0, 228.0);
    h.hover_at(end);
    // Wait only for the one-time background preparation. Later pointer frames are immediate.
    h.run_steps(1);
    rendered_ink(&mut h, 160.0, 242.0);
    let end = at(&h, 220.0, 236.0);
    h.hover_at(end);
    h.run_steps(2);
    let resized = h.render().unwrap();
    assert!(pixel_at(&h, &resized, 200.0, 236.0)[..3].iter().all(|v| *v < 60), "ink resizes before release");
    let assert_green = |v: [u8; 4]| {
        assert!(v[1] > 230 && i32::from(v[1]) > i32::from(v[0]) + 10 && i32::from(v[1]) > i32::from(v[2]) + 10, "page content is preserved: {v:?}")
    };
    assert_green(pixel_at(&h, &resized, 90.0, 250.0));
    assert!(has_blue_near(&resized, at(&h, 220.0, 266.0)), "resizing keeps its handles visible");
    let doc = h.state().session.get(h.state().views[0].id).unwrap();
    assert_eq!(doc.edit_generation(), generation, "pointer frames never edit the document");
    assert_eq!(doc.can_undo(), Some("Add signature"));
    assert_rect(doc.info.annotations[0].rect, [40.0, 234.0, 136.0, 266.0]);
    if let Ok(dir) = std::env::var("PDFCRAFT_SHOTS") {
        resized.save(format!("{dir}/image-signature-live-resize.png")).unwrap();
    }
    h.drop_at(end);
    h.run_steps(3);
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Resize comment"));

    let start = at(&h, 130.0, 236.0);
    h.drag_at(start);
    h.run_steps(1);
    for (x, y) in [(140.0, 180.0), (150.0, 140.0)] {
        h.hover_at(at(&h, x, y));
        h.run_steps(2);
        let moving = h.render().unwrap();
        assert!(pixel_at(&h, &moving, x, y)[..3].iter().all(|v| *v < 60), "ink follows each pointer position");
        assert_green(pixel_at(&h, &moving, 130.0, 236.0));
        assert_green(pixel_at(&h, &moving, x - 85.0, y));
        for (hx, hy) in [
            (x - 90.0, y + 30.0),
            (x, y + 30.0),
            (x + 90.0, y + 30.0),
            (x + 90.0, y),
            (x + 90.0, y - 30.0),
            (x, y - 30.0),
            (x - 90.0, y - 30.0),
            (x - 90.0, y),
        ] {
            assert!(!has_blue_near(&moving, at(&h, hx, hy)), "moving hides the frame and all eight handles");
        }
        assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Resize comment"));
        if y == 140.0
            && let Ok(dir) = std::env::var("PDFCRAFT_SHOTS")
        {
            moving.save(format!("{dir}/image-signature-live-move.png")).unwrap();
        }
    }
    let end = at(&h, 150.0, 140.0);
    h.drop_at(end);
    h.run_steps(2);
    let released = h.render().unwrap();
    assert!(has_blue_near(&released, at(&h, 240.0, 170.0)), "the frame returns immediately on release");
    let doc = h.state().session.get(h.state().views[0].id).unwrap();
    assert_eq!(doc.can_undo(), Some("Move comment"));
    assert_eq!(doc.edit_generation(), generation + 2, "one edit for each completed gesture");
    assert_rect(doc.info.annotations[0].rect, [60.0, 110.0, 240.0, 170.0]);
    assert_eq!(h.state().views[0].session.comments.primary(), Some((0, 0)));
    if let Ok(dir) = std::env::var("PDFCRAFT_SHOTS") {
        released.save(format!("{dir}/image-signature-move-released.png")).unwrap();
    }
    undo(&mut h);
    undo(&mut h);
    h.run_steps(3);
    assert_rect(h.state().session.get(h.state().views[0].id).unwrap().info.annotations[0].rect, [40.0, 234.0, 136.0, 266.0]);
    rendered_ink(&mut h, 90.0, 250.0);
    click(&mut h, 90.0, 250.0);
    h.drag_at(at(&h, 90.0, 250.0));
    h.run_steps(1);
    h.hover_at(at(&h, 150.0, 180.0));
    rendered_ink(&mut h, 150.0, 180.0);
    assert_eq!(
        h.state().session.get(h.state().views[0].id).unwrap().can_undo(),
        Some("Add signature"),
        "moving preview is still read-only before Escape"
    );
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(h.state().views[0].session.comments.tool.in_flight().is_none(), "Escape must cancel before releasing the mouse");
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Add signature"), "Escape must not commit");
    h.drop_at(at(&h, 150.0, 180.0));
    h.run_steps(2);
    assert!(h.state().views[0].session.comments.tool.in_flight().is_none());
    let doc = h.state().session.get(h.state().views[0].id).unwrap();
    assert_eq!(doc.can_undo(), Some("Add signature"), "Escape cancels without an undo step");
    assert_rect(doc.info.annotations[0].rect, [40.0, 234.0, 136.0, 266.0]);
    let cancelled = h.render().unwrap();
    assert_green(pixel_at(&h, &cancelled, 150.0, 180.0));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn embedded_image_signature_live_gestures_follow_document_and_view_rotation() {
    let _gpu = gpu();
    let path = signature_file("live-rotations");
    let image = saved_image(&path);
    for (document_rotation, view_rotation) in [(90, 0), (0, 90), (90, 270)] {
        let mut h = harness();
        let ctx_h = h.ctx.clone();
        h.state_mut().apply_edit(&ctx_h, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: document_rotation });
        while h.state().views[0].view.rotation != view_rotation {
            turn_view(&mut h);
        }
        h.set_option("zoom", "100").unwrap();
        h.state_mut().signature = Some(SavedSig::Image(image.clone()));
        h.execute("sign.fill.signature");
        h.run_steps(3);
        // Points as displayed (from the shown page's bottom-left, y up). The signature is upright
        // as displayed, so the same points apply whatever the page's /Rotate.
        let page = h.state().session.get(h.state().views[0].id).unwrap().info.pages[0].clone();
        let u = |x: f32, y: f32| page.view_to_user(x, page.height - y);
        let [x, y] = u(40.0, 250.0);
        click(&mut h, x, y);
        let [x, y] = u(90.0, 250.0);
        rendered_ink(&mut h, x, y);
        // Future placements may use a different signature: drag the PDF's embedded image.
        h.state_mut().signature = None;
        let [x, y] = u(136.0, 234.0);
        h.drag_at(at(&h, x, y));
        h.run_steps(1);
        let [x, y] = u(184.0, 228.0);
        let end = at(&h, x, y);
        h.hover_at(end);
        let [x, y] = u(160.0, 242.0);
        h.run_steps(1);
        rendered_ink(&mut h, x, y);
        let pixels = h.render().unwrap();
        let [x, y] = u(58.0, 254.0);
        assert!(pixel_at(&h, &pixels, x, y)[..3].iter().all(|v| *v < 60), "asymmetric ink stays upright during resize");
        let [x, y] = u(58.0, 230.0);
        assert!(pixel_at(&h, &pixels, x, y)[..3].iter().all(|v| *v > 240), "alpha isn't flipped");
        h.drop_at(end);
        h.run_steps(3);
        // The corner keeps the image's 3:1 as displayed: 144 x 48 from the top-left corner.
        let ([x0, y0], [x1, y1]) = (u(40.0, 218.0), u(184.0, 266.0));
        let rect = h.state().session.get(h.state().views[0].id).unwrap().info.annotations[0].rect;
        let want = [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)];
        assert!(rect.iter().zip(want).all(|(a, b)| (a - b).abs() < 0.01), "{rect:?} != {want:?}");
        let [x, y] = u(112.0, 242.0);
        h.drag_at(at(&h, x, y));
        h.run_steps(1);
        let [x, y] = u(132.0, 142.0);
        let target = at(&h, x, y);
        h.hover_at(target);
        h.run_steps(2);
        let pixels = h.render().unwrap();
        assert!(pixel_at(&h, &pixels, x, y)[..3].iter().all(|v| *v < 60), "image moves before release");
        let [x, y] = u(160.0, 242.0);
        assert!(pixel_at(&h, &pixels, x, y)[..3].iter().all(|v| *v > 240), "old ink is removed during movement");
        let [x, y] = u(204.0, 166.0);
        let corner = at(&h, x, y);
        assert!(!has_blue_near(&pixels, corner));
        h.drop_at(target);
        h.run_steps(2);
        assert!(has_blue_near(&h.render().unwrap(), corner));
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn image_signature_corners_restore_original_aspect_after_edge_resize_and_reopen() {
    let _gpu = gpu();
    let path = signature_file("aspect");
    let image = saved_image(&path);
    let mut h = harness();
    h.state_mut().signature = Some(SavedSig::Image(image));
    h.execute("sign.fill.signature");
    click(&mut h, 70.0, 210.0);
    // Side handles stretch just their own axis; pointer movement along the other axis is ignored.
    for (from, to, expected) in
        [([166.0, 210.0], [214.0, 216.0], [70.0, 194.0, 214.0, 226.0]), ([142.0, 194.0], [150.0, 170.0], [70.0, 170.0, 214.0, 226.0])]
    {
        h.drag_at(at(&h, from[0], from[1]));
        h.run_steps(1);
        let end = at(&h, to[0], to[1]);
        h.hover_at(end);
        h.run_steps(2);
        h.drop_at(end);
        h.run_steps(3);
        assert_rect(h.state().session.get(h.state().views[0].id).unwrap().info.annotations[0].rect, expected);
    }
    let bytes = h.state().session.get(h.state().views[0].id).unwrap().bytes.clone();
    // Reopened PDFs have no saved signature setting: each corner must use the embedded 3:1 image,
    // rather than the stretched 144:56 annotation rectangle.
    for (hx, hy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        let mut h = harness_bytes(&bytes);
        assert!(h.state().signature.is_none());
        h.state_mut().views[0].session.comments.select_only((0, 0));
        h.run_steps(3);
        rendered_ink(&mut h, 142.0, 198.0);
        let generation = h.state().session.get(h.state().views[0].id).unwrap().edit_generation();
        let (x, y) = (if hx < 0.0 { 70.0 } else { 214.0 }, if hy < 0.0 { 170.0 } else { 226.0 });
        h.drag_at(at(&h, x, y));
        h.run_steps(1);
        let end = at(&h, x + hx * 10.0, y + hy * 10.0);
        h.hover_at(end);
        h.run_steps(1);
        let expected = [
            if hx < 0.0 { 16.0 } else { 70.0 },
            if hy < 0.0 { 160.0 } else { 170.0 },
            if hx < 0.0 { 214.0 } else { 268.0 },
            if hy < 0.0 { 226.0 } else { 236.0 },
        ];
        let ink_x = expected[0] + 198.0 * if hx < 0.0 { 0.15 } else { 0.85 };
        let pixels = rendered_ink(&mut h, ink_x, (expected[1] + expected[3]) / 2.0);
        assert!(has_blue_near(&pixels, at(&h, if hx < 0.0 { expected[0] } else { expected[2] }, if hy < 0.0 { expected[1] } else { expected[3] })));
        let doc = h.state().session.get(h.state().views[0].id).unwrap();
        assert_eq!(doc.edit_generation(), generation, "corner previews remain read-only");
        assert_rect(doc.info.annotations[0].rect, [70.0, 170.0, 214.0, 226.0]);
        if hx > 0.0
            && hy < 0.0
            && let Ok(dir) = std::env::var("PDFCRAFT_SHOTS")
        {
            pixels.save(format!("{dir}/image-signature-aspect-corner.png")).unwrap();
        }
        h.drop_at(end);
        h.run_steps(3);
        let doc = h.state().session.get(h.state().views[0].id).unwrap();
        assert_rect(doc.info.annotations[0].rect, expected);
        assert_eq!(doc.edit_generation(), generation + 1);
        assert_eq!(doc.can_undo(), Some("Resize comment"));
        assert!(((expected[2] - expected[0]) / (expected[3] - expected[1]) - 3.0).abs() < 0.001);
        undo(&mut h);
        h.run_steps(3);
        assert_rect(h.state().session.get(h.state().views[0].id).unwrap().info.annotations[0].rect, [70.0, 170.0, 214.0, 226.0]);
        redo(&mut h);
        h.run_steps(3);
        assert_rect(h.state().session.get(h.state().views[0].id).unwrap().info.annotations[0].rect, expected);
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn image_import_cancel_clear_and_errors_preserve_saved_signatures() {
    let path = signature_file("cancel");
    let files = Arc::new(ScriptedFiles::held());
    let mut h = harness_with_files(&files);
    let original = SavedSig::Typed("Ada Lovelace".into());
    h.state_mut().signature = Some(original.clone());
    h.execute("sign.fill.signature.change");
    h.run_steps(2);
    h.get_by_label("Image").click();
    h.run_steps(2);
    files.queue(Picked::Cancelled);
    h.get_by_label("Browse…").click();
    settle_decode(&mut h);
    assert!(!can_apply(&h), "a cancelled pick loads nothing");
    browse_image(&mut h, &files, &path);
    assert!(can_apply(&h), "the picked image is loaded");
    files.queue(Picked::Contents(vec![("broken.png".into(), b"broken PNG".to_vec())]));
    h.get_by_label("Browse…").click();
    settle_decode(&mut h);
    assert!(can_apply(&h), "an invalid replacement keeps the previous draft");
    assert!(shows(&h, "That image could not be read."), "the pad says why");
    h.get_by_label("Clear").click();
    h.run_steps(2);
    assert!(!can_apply(&h));
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert_eq!(h.state().signature, Some(original));
    assert!(items(&h).is_empty());
    // A picker answer from a closed dialog cannot populate a later draft.
    h.execute("sign.fill.signature.change");
    h.run_steps(2);
    h.get_by_label("Image").click();
    h.run_steps(2);
    h.get_by_label("Browse…").click();
    h.run_steps(1);
    assert_eq!(files.open(), 1, "the pick waits for its answer");
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert!(!pad_open(&h));
    files.answer(Picked::Contents(vec![("signature".into(), std::fs::read(&path).unwrap())]));
    h.run_steps(3);
    h.execute("sign.fill.signature.change");
    h.run_steps(2);
    h.get_by_label("Image").click();
    settle_decode(&mut h);
    assert!(!can_apply(&h), "the stale answer did not reach the new draft");
    // Malformed settings are ignored while legacy typed/drawn values still restore.
    let mut again = crate::control::new_app();
    again.set_option(&egui::Context::default(), "language", "en").unwrap();
    restore_json(&mut again, r#"{"signature_text":"Ada","signature_image":{"Image":"bm90IGFuIGltYWdl"},"initials":{"Image":"%%%"}}"#);
    assert_eq!(again.signature, Some(SavedSig::Typed("Ada".into())));
    assert!(again.initials.is_none());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn flatten_on_save_is_off_by_default_and_round_trips() {
    let mut app = crate::control::new_app();
    assert!(!app.flatten_fill_sign_on_save);
    app.set_option(&egui::Context::default(), "flatten-fill-sign", "true").unwrap();
    assert!(app.set_option(&egui::Context::default(), "flatten-fill-sign", "yes").is_err());
    let mut restored = crate::control::new_app();
    restore_from(&mut restored, &app);
    assert!(restored.flatten_fill_sign_on_save);
    restored.set_option(&egui::Context::default(), "flatten-fill-sign", "false").unwrap();
    let mut again = crate::control::new_app();
    restore_from(&mut again, &restored);
    assert!(!again.flatten_fill_sign_on_save);
}

#[test]
fn saving_flattens_fill_and_sign_only_when_the_preference_is_on() {
    use pdfcraft_engine::{Edit, FillMark, NewAnnotation, Shape, Style};
    use pdfcraft_ui_egui::SaveTarget;
    use pdfcraft_ui_egui::fill_sign::TypeBox;

    let dir = std::path::PathBuf::from("/pdfcraft-fill-flatten");
    let mark = Edit::AddAnnotation(NewAnnotation {
        page: 0,
        shape: Shape::Mark { rect: [40.0, 80.0, 56.0, 96.0], mark: FillMark::Check },
        style: Style::default_for(&Shape::Mark { rect: [0.0, 0.0, 12.0, 12.0], mark: FillMark::Check }),
        contents: String::new(),
        author: "Ada".into(),
    });

    let kept = dir.join("kept.pdf");
    let files = Arc::new(ScriptedFiles::new());
    let mut app = crate::control::app_with_files(files.clone());
    app.open_bytes(&egui::Context::default(), "form.pdf", Some(kept.to_string_lossy().into_owned()), FIXTURE.to_vec()).unwrap();
    let id = app.views[0].id;
    app.session.apply(id, mark).unwrap();
    app.views[0].session.fill_text = Some(TypeBox {
        page: 0,
        at: [40.0, 200.0],
        text: "Typed".into(),
        size: 10.0,
        color: [0.0; 3],
        border: pdfcraft_ui_egui::fill_sign::TextBorder::NONE,
        focus: false,
    });
    assert!(app.save_active(&egui::Context::default(), SaveTarget::InPlace));
    let mut reopened = crate::control::new_app();
    reopened.open_bytes(&egui::Context::default(), "kept.pdf", None, files.file(&kept).expect("the save wrote the file")).unwrap();
    let texts: Vec<String> =
        reopened.session.get(reopened.views[0].id).unwrap().info.annotations.iter().map(|a| a.contents.clone().unwrap_or_default()).collect();
    assert!(texts.iter().any(|t| t == "Typed"), "an open type box is saved: {texts:?}");
    assert_eq!(reopened.session.get(reopened.views[0].id).unwrap().info.annotations.len(), 2, "the check mark stays too");

    app.views[0].session.fill_text = Some(TypeBox {
        page: 0,
        at: [40.0, 160.0],
        text: "More".into(),
        size: 10.0,
        color: [0.0; 3],
        border: pdfcraft_ui_egui::fill_sign::TextBorder::NONE,
        focus: false,
    });
    app.flatten_fill_sign_on_save = true;
    assert!(app.save_active(&egui::Context::default(), SaveTarget::InPlace));
    let mut flattened = crate::control::new_app();
    flattened.open_bytes(&egui::Context::default(), "flat.pdf", None, files.file(&kept).expect("the save wrote the file")).unwrap();
    let left = &flattened.session.get(flattened.views[0].id).unwrap().info.annotations;
    assert!(left.is_empty(), "text and the check mark are page content: {left:?}");
}

/// Issue #299: drawn and typed signatures and initials are upright as displayed on pages with
/// `/Rotate` 90, 180 and 270, anchored at the displayed point, exactly as on an unturned page.
#[test]
fn drawn_and_typed_signatures_stay_upright_on_rotated_pages() {
    // An "L": a long stroke along the bottom and a tick up at its left end, so a turned or
    // mirrored signature shows.
    let drawn = SavedSig::Drawn(vec![vec![[0.0, 0.0], [1.0, 0.0]], vec![[0.0, 0.0], [0.0, 0.2]]]);
    let typed = SavedSig::Typed("Ada Lovelace".into());
    // (signature, initials, displayed left-centre point)
    let cases = [(&drawn, false, [40.0_f32, 60.0]), (&typed, false, [40.0, 140.0]), (&typed, true, [40.0, 230.0])];
    let ink_mask = |rotation: i64| {
        let mut session = pdfcraft_engine::Session::new();
        let blank = session.create_blank(300.0, 400.0, 1).unwrap();
        let id = session.open_new("form.pdf", blank).unwrap();
        if rotation != 0 {
            session.apply(id, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: rotation }).unwrap();
        }
        let page = session.get(id).unwrap().info.pages[0].clone();
        assert_eq!(i64::from(page.rotation), rotation);
        for (sig, initials, shown) in cases {
            let at = page.view_to_user(shown[0], shown[1]);
            let edit = pdfcraft_ui_egui::fill_sign::place(0, &page, [f64::from(at[0]), f64::from(at[1])], sig, initials, "Ada").unwrap();
            session.apply(id, edit).unwrap();
        }
        let annotations = &session.get(id).unwrap().info.annotations;
        assert_eq!(annotations.len(), cases.len());
        let bytes = session.get(id).unwrap().bytes.clone();
        let mut renderer = pdfcraft_render::PageRenderer::new(bytes, Default::default());
        let out = renderer.render(pdfcraft_render::RenderRequest { page: 0, scale: 1.0, ..Default::default() });
        assert!(out.error.is_none(), "{:?}", out.error);
        let shown_size = if rotation % 180 == 0 { (300, 400) } else { (400, 300) };
        assert_eq!((out.width, out.height), shown_size, "rendered as displayed");
        let dark: Vec<bool> = out.rgba.as_chunks::<4>().0.iter().map(|p| p[..3].iter().all(|v| *v < 128)).collect();
        // The displayed ink box of each signature, in the band around its point.
        let boxes: Vec<[u32; 4]> = cases
            .iter()
            .map(|(_, _, shown)| {
                let (top, bottom) = ((shown[1] - 35.0) as u32, (shown[1] + 35.0) as u32);
                let mut b = [u32::MAX, u32::MAX, 0, 0];
                for y in top..bottom {
                    for x in 0..out.width {
                        if dark[(y * out.width + x) as usize] {
                            b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
                        }
                    }
                }
                assert!(b[0] <= b[2], "rotation {rotation}: no ink near {shown:?}");
                b
            })
            .collect();
        (dark, out.width, boxes)
    };
    let (upright, _, upright_boxes) = ink_mask(0);
    for (b, (sig, _, shown)) in upright_boxes.iter().zip(cases) {
        assert!(b[2] - b[0] > b[3] - b[1], "{sig:?} reads across: {b:?}");
        assert!(b[0].abs_diff(shown[0] as u32) <= 2, "{sig:?} starts at the point: {b:?}");
        assert!(b[1] < shown[1] as u32 && b[3] > shown[1] as u32, "{sig:?} is centred on the point: {b:?}");
    }
    // The drawn "L" has its tick at the top left, not mirrored or upside down.
    let at = |x: u32, y: u32| upright[(y * 300 + x) as usize];
    let b = upright_boxes[0];
    assert!(at(b[0] + 1, b[1] + 2) && !at(b[2] - 2, b[1] + 2) && at(b[2] - 2, b[3] - 1), "the drawn L is upright: {b:?}");
    for rotation in [90, 180, 270] {
        let (dark, width, boxes) = ink_mask(rotation);
        for (got, want) in boxes.iter().zip(&upright_boxes) {
            assert!(got.iter().zip(want).all(|(g, w)| g.abs_diff(*w) <= 1), "rotation {rotation}: displayed box {got:?}, upright {want:?}");
        }
        // Compare where both pages are shown (the turned ones are 400 x 300).
        let (mut differ, mut ink) = (0, 0);
        for y in 0..300 {
            for x in 0..300 {
                let (want, got) = (upright[(y * 300 + x) as usize], dark[(y * width + x) as usize]);
                ink += usize::from(want);
                differ += usize::from(want != got);
            }
        }
        assert!(differ * 10 < ink, "rotation {rotation}: {differ} of {ink} ink pixels differ from the unturned page");
    }
}

/// Press, move in steps and release, from one user-space point to another.
fn drag(h: &mut Harness<'static, PdfCraftApp>, from: (f32, f32), to: (f32, f32)) {
    let (a, b) = (at(h, from.0, from.1), at(h, to.0, to.1));
    h.hover_at(a);
    h.run_steps(1);
    h.drag_at(a);
    h.run_steps(1);
    for k in 1..=5 {
        h.hover_at(a + (b - a) * (k as f32 / 5.0));
        h.run_steps(1);
    }
    h.drop_at(b);
    h.run_steps(4);
}

fn rects(h: &Harness<'static, PdfCraftApp>) -> Vec<[f32; 4]> {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().info.annotations.iter().map(|a| a.rect).collect()
}

/// A placed signature moves while a Fill & Sign tool is still active (as in Acrobat), instead of
/// the drag doing nothing and a click stamping another mark on top of it. Elsewhere on the page
/// the tool still places its mark.
#[test]
fn fill_and_sign_tools_pick_up_placed_marks_to_move_and_resize_them() {
    for tool in ["sign.fill.signature", "sign.fill.initials", "sign.fill.text", "sign.fill.check", "sign.fill.date"] {
        let mut h = harness();
        h.state_mut().signature = Some(SavedSig::Typed("Ada Lovelace".into()));
        h.state_mut().initials = Some(SavedSig::Typed("AL".into()));
        h.execute("sign.fill.signature");
        click(&mut h, 40.0, 250.0);
        let [placed] = rects(&h)[..] else { panic!("one signature: {:?}", rects(&h)) };
        assert!(h.execute(tool));
        h.run_steps(2);
        let mid = ((placed[0] + placed[2]) / 2.0, (placed[1] + placed[3]) / 2.0);
        h.hover_at(at(&h, mid.0, mid.1));
        h.run_steps(2);
        assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::Move, "{tool}: a placed mark can be picked up");

        drag(&mut h, mid, (mid.0 + 40.0, mid.1 - 60.0));
        assert!(matches!(h.state().quick_tool, QuickTool::Fill(_)), "{tool}: the tool stays");
        let moved = [placed[0] + 40.0, placed[1] - 60.0, placed[2] + 40.0, placed[3] - 60.0];
        let [now] = rects(&h)[..] else { panic!("{tool}: still one mark: {:?}", rects(&h)) };
        assert_rect(now, moved);
        let doc = h.state().session.get(h.state().views[0].id).unwrap();
        assert_eq!(doc.can_undo(), Some("Move comment"), "{tool}");
        assert_eq!(h.state().views[0].session.comments.primary(), Some((0, 0)), "{tool}");

        // The selected signature's handle resizes it.
        let corner = at(&h, moved[2], moved[1]);
        h.hover_at(corner);
        h.run_steps(2);
        h.drag_at(corner);
        h.run_steps(1);
        h.hover_at(corner + egui::vec2(30.0, 10.0));
        h.run_steps(2);
        h.drop_at(corner + egui::vec2(30.0, 10.0));
        h.run_steps(3);
        assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Resize comment"), "{tool}");
        assert_eq!(rects(&h).len(), 1, "{tool}");

        // A click on it selects it rather than placing another mark on top.
        h.state_mut().views[0].session.comments.clear_selection();
        let r = rects(&h)[0];
        click(&mut h, (r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
        assert_eq!(rects(&h).len(), 1, "{tool}: no mark stamped on the signature");
        assert_eq!(h.state().views[0].session.comments.primary(), Some((0, 0)), "{tool}");
        assert!(h.state().views[0].session.fill_text.is_none(), "{tool}: no text box opened on the signature");
    }
    // A drawn signature in a saved and reopened file (read by the viewer, not the editor).
    let mut h = harness();
    h.state_mut().signature = Some(SavedSig::Drawn(vec![vec![[0.05, 0.1], [0.3, 0.2], [0.55, 0.05], [0.95, 0.15]]]));
    h.execute("sign.fill.signature");
    click(&mut h, 40.0, 250.0);
    let saved = h.state().session.save_bytes(h.state().views[0].id).unwrap();
    let mut h = harness_bytes(&saved);
    let [placed] = rects(&h)[..] else { panic!("one signature: {:?}", rects(&h)) };
    h.execute("sign.fill.check");
    let mid = ((placed[0] + placed[2]) / 2.0, (placed[1] + placed[3]) / 2.0);
    drag(&mut h, mid, (mid.0 + 40.0, mid.1 - 60.0));
    assert_rect(rects(&h)[0], [placed[0] + 40.0, placed[1] - 60.0, placed[2] + 40.0, placed[3] - 60.0]);

    // Away from Fill & Sign marks, the tool still places (checks stay repeatable), also on other
    // comments.
    let mut h = harness();
    h.state_mut().signature = Some(SavedSig::Typed("Ada Lovelace".into()));
    h.execute("sign.fill.signature");
    click(&mut h, 40.0, 250.0);
    let id = h.state().views[0].id;
    let square = pdfcraft_engine::Shape::Rectangle { rect: [100.0, 80.0, 200.0, 140.0] };
    let style = pdfcraft_engine::Style::default_for(&square);
    let new = pdfcraft_engine::NewAnnotation { page: 0, shape: square, style, contents: String::new(), author: "Ada".into() };
    h.state_mut().session.apply(id, pdfcraft_engine::Edit::AddAnnotation(new)).unwrap();
    h.run_steps(2);
    h.execute("sign.fill.check");
    click(&mut h, 60.0, 100.0);
    click(&mut h, 150.0, 110.0);
    assert_eq!(items(&h).len(), 4, "{:?}", items(&h));
    assert_eq!(h.state().quick_tool, QuickTool::Fill(FillTool::Check));
}

/// A drawn signature resizes from its handles like typed and image ones: the corners keep its
/// shape, the strokes scale with it, and the size survives undo, redo and saving.
#[test]
fn drawn_signatures_resize_from_their_corners() {
    let mut h = harness();
    h.state_mut().signature = Some(SavedSig::Drawn(vec![vec![[0.05, 0.1], [0.3, 0.2], [0.55, 0.05], [0.95, 0.15]]]));
    h.execute("sign.fill.signature");
    click(&mut h, 40.0, 250.0);
    let [placed] = rects(&h)[..] else { panic!("one signature: {:?}", rects(&h)) };
    assert_eq!(h.state().quick_tool, QuickTool::Select);
    assert_eq!(h.state().views[0].session.comments.primary(), Some((0, 0)), "the placed signature is selected");

    // The top-right corner handle, dragged out and a little up.
    let corner = at(&h, placed[2], placed[3]);
    h.hover_at(corner);
    h.run_steps(2);
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::ResizeNeSw);
    h.drag_at(corner);
    h.run_steps(1);
    for k in 1..=10 {
        h.hover_at(corner + egui::vec2(6.0 * k as f32, -(k as f32)));
        h.run_steps(1);
    }
    h.drop_at(corner + egui::vec2(60.0, -10.0));
    h.run_steps(4);
    let doc = h.state().session.get(h.state().views[0].id).unwrap();
    assert_eq!(doc.can_undo(), Some("Resize comment"));
    let [now] = rects(&h)[..] else { panic!("still one signature: {:?}", rects(&h)) };
    let ratio = |r: [f32; 4]| (r[2] - r[0]) / (r[3] - r[1]);
    assert!(now[2] - now[0] > placed[2] - placed[0] + 20.0, "wider: {placed:?} -> {now:?}");
    assert!((ratio(now) - ratio(placed)).abs() < 0.02 * ratio(placed), "same shape: {placed:?} -> {now:?}");
    assert!((now[0] - placed[0]).abs() < 0.01 && (now[1] - placed[1]).abs() < 0.01, "the opposite corner stays put");

    let id = h.state().views[0].id;
    h.state_mut().session.undo(id).unwrap();
    assert_rect(rects(&h)[0], placed);
    h.state_mut().session.redo(id).unwrap();
    assert_rect(rects(&h)[0], now);
    let saved = h.state().session.save_bytes(id).unwrap();
    let h = harness_bytes(&saved);
    assert_rect(rects(&h)[0], now);
}

/// Whether black ink is drawn within 3 pixels of `p`.
fn ink_near(pixels: &image::RgbaImage, p: Pos2) -> bool {
    (-3..=3).any(|dy| (-3..=3).any(|dx| pixels.get_pixel((p.x as i32 + dx) as u32, (p.y as i32 + dy) as u32).0[..3].iter().all(|v| *v < 60)))
}

/// A Fill & Sign line, like every stamp, is stretched with its frame while a handle is dragged:
/// it grows and shrinks with the pointer instead of keeping its old length until the release.
#[test]
fn lines_grow_and_shrink_with_their_handles_before_release() {
    let _gpu = gpu();
    for (document_rotation, view_rotation) in [(0, 0), (90, 0), (0, 90), (90, 270)] {
        let rot = (document_rotation, view_rotation);
        // A line runs across the page as /Rotate displays it (#3781): along user y on a quarter turn.
        // `d` is the distance along the line from the click at (100, 250).
        let along =
            |h: &Harness<'static, PdfCraftApp>, d: f32| if document_rotation == 90 { at(h, 100.0, 250.0 + d) } else { at(h, 100.0 + d, 250.0) };
        let line = |from: f32, to: f32| {
            if document_rotation == 90 { [98.0, 250.0 + from, 102.0, 250.0 + to] } else { [100.0 + from, 248.0, 100.0 + to, 252.0] }
        };
        let mut h = harness();
        if document_rotation != 0 {
            let ctx_h = h.ctx.clone();
            h.state_mut().apply_edit(&ctx_h, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: document_rotation });
        }
        while h.state().views[0].view.rotation != view_rotation {
            turn_view(&mut h);
        }
        h.run_steps(3);
        assert!(h.execute("sign.fill.line"));
        click(&mut h, 100.0, 250.0);
        assert_rect(rects(&h)[0], line(-18.0, 18.0));
        // In line mode a click on the line picks it up, with its handles.
        click(&mut h, 100.0, 250.0);
        assert_eq!(h.state().views[0].session.comments.primary(), Some((0, 0)), "{rot:?}");
        let generation = h.state().session.get(h.state().views[0].id).unwrap().edit_generation();

        // The handle at the line's end, dragged out to 60 from the middle.
        h.drag_at(along(&h, 18.0));
        h.run_steps(1);
        h.hover_at(along(&h, 60.0));
        h.run_steps(1);
        // Wait only for the one-time layers. Later pointer frames are immediate.
        let mut grown = h.render().unwrap();
        for _ in 0..50 {
            if ink_near(&grown, along(&h, 50.0)) {
                break;
            }
            h.run_steps(2);
            std::thread::sleep(std::time::Duration::from_millis(10));
            grown = h.render().unwrap();
        }
        assert!(ink_near(&grown, along(&h, 50.0)), "{rot:?}: the line grows with the handle before release");
        assert!(ink_near(&grown, along(&h, -10.0)), "{rot:?}: from where it starts");

        // Back in, shorter than it was, still without letting go.
        h.hover_at(along(&h, 0.0));
        h.run_steps(2);
        let shrunk = h.render().unwrap();
        assert!(ink_near(&shrunk, along(&h, -10.0)), "{rot:?}: the line still starts where it did");
        for d in [10.0, 50.0] {
            assert!(!ink_near(&shrunk, along(&h, d)), "{rot:?}: the line shrinks with the handle before release (ink at {d})");
        }
        let doc = h.state().session.get(h.state().views[0].id).unwrap();
        assert_eq!((doc.edit_generation(), doc.can_undo()), (generation, Some("Add line")), "{rot:?}: pointer frames never edit the document");
        if let Ok(dir) = std::env::var("PDFCRAFT_SHOTS") {
            grown.save(format!("{dir}/line-live-grow-{document_rotation}-{view_rotation}.png")).unwrap();
            shrunk.save(format!("{dir}/line-live-shrink-{document_rotation}-{view_rotation}.png")).unwrap();
        }
        h.drop_at(along(&h, 0.0));
        h.run_steps(3);
        let doc = h.state().session.get(h.state().views[0].id).unwrap();
        assert_eq!(doc.can_undo(), Some("Resize comment"), "{rot:?}");
        let [now] = rects(&h)[..] else { panic!("{rot:?}: still one line: {:?}", rects(&h)) };
        assert!(now.iter().zip(line(-18.0, 0.0)).all(|(a, b)| (a - b).abs() < 0.01), "{rot:?}: {now:?}");
    }
}

/// A signature placed over a form field is picked up by the Select tool: the field under it
/// doesn't take the press. Away from the signature, a click still fills the field.
#[test]
fn select_tool_picks_up_a_signature_over_a_form_field() {
    let mut h = harness_bytes(include_bytes!("../data/form.pdf"));
    h.state_mut().signature = Some(SavedSig::Typed("Ada Lovelace".into()));
    h.execute("sign.fill.signature");
    click(&mut h, 150.0, 270.0);
    let [placed] = rects(&h)[..] else { panic!("one signature: {:?}", rects(&h)) };
    // Onto the `city` text field (user space y 180..200).
    let mid_y = (placed[1] + placed[3]) / 2.0;
    let id = h.state().views[0].id;
    let index = h.state().session.get(id).unwrap().info.annotations[0].index;
    let dy = (190.0 - mid_y) as f64;
    h.state_mut().session.apply(id, pdfcraft_engine::Edit::MoveAnnotation { page: 0, index, dx: 0.0, dy }).unwrap();
    h.state_mut().views[0].session.comments.clear_selection();
    h.state_mut().quick_tool = QuickTool::Select;
    h.run_steps(3);
    let [over] = rects(&h)[..] else { panic!("one signature: {:?}", rects(&h)) };
    let mid = ((over[0] + over[2]) / 2.0, (over[1] + over[3]) / 2.0);

    h.hover_at(at(&h, mid.0, mid.1));
    h.run_steps(2);
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::Move);
    // A hand's drag: a few points per frame, so it starts while still over the field.
    let (a, b) = (at(&h, mid.0, mid.1), at(&h, mid.0, mid.1 + 80.0));
    h.drag_at(a);
    h.run_steps(1);
    for k in 1..=40 {
        h.hover_at(a + (b - a) * (k as f32 / 40.0));
        h.run_steps(1);
    }
    h.drop_at(b);
    h.run_steps(4);
    assert_rect(rects(&h)[0], [over[0], over[1] + 80.0, over[2], over[3] + 80.0]);
    assert!(h.state().views[0].session.forms.focus.is_none(), "the field under the signature isn't focused");

    // Clear of the signature, the field still takes the click.
    click(&mut h, 270.0, 190.0);
    assert_eq!(h.state().views[0].session.forms.focus.as_ref().map(|f| f.name.as_str()), Some("city"));
}

/// A synthetic phone photo of a signature on paper: a dark table along the left edge, a sheet
/// with a lighting gradient and a soft shadow, and a ballpoint-blue signature in two words.
fn signature_photo(test: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("pdfcraft-signature-photo-{test}-{}.png", std::process::id()));
    let image = image::RgbImage::from_fn(800, 600, |x, y| {
        if x < 60 {
            return image::Rgb([70, 55, 45]);
        }
        let light = 235.0 - x as f32 * 0.04 - if (500..700).contains(&x) && y > 380 { 45.0 } else { 0.0 };
        let (fx, fy) = (x as f32, y as f32);
        let wave = |x0: f32, x1: f32| (x0..x1).contains(&fx) && (fy - (300.0 + 25.0 * ((fx - x0) / 18.0).sin())).abs() < 3.0;
        if wave(180.0, 380.0) || wave(420.0, 640.0) {
            image::Rgb([30, 40, 110])
        } else {
            let v = light.clamp(0.0, 255.0) as u8;
            image::Rgb([v, v, v.saturating_sub(6)])
        }
    });
    image.save(&path).unwrap();
    path
}

/// Choose photo… on the pad's Scan tab, answered with the file `path`, then wait for the scan.
fn choose_photo(h: &mut Harness<'static, PdfCraftApp>, files: &ScriptedFiles, path: &std::path::Path) -> bool {
    files.queue(Picked::Contents(vec![("photo".into(), std::fs::read(path).unwrap())]));
    h.get_by_label("Choose photo…").click();
    h.run_steps(3);
    settle_scan(h)
}

/// Scans run on a worker thread: step frames, bounded, until the pad can apply a result. Says whether it can.
fn settle_scan(h: &mut Harness<'static, PdfCraftApp>) -> bool {
    for _ in 0..400 {
        h.run_steps(1);
        if can_apply(h) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    false
}

/// Draw a new scan frame on the pad's photo from one point of the photo (800 × 600) to another.
fn drag_frame(h: &mut Harness<'static, PdfCraftApp>, from: (f32, f32), to: (f32, f32)) -> bool {
    let area = h.get_by_label("Photo").rect();
    let mut view = craft::behavior::CanvasView::new(area, craft::behavior::kurbo::Point::ORIGIN, 1.0);
    let target = craft::ui::canvas::ViewTarget::new(craft::behavior::kurbo::Size::new(800.0, 600.0));
    craft::ui::canvas::ViewCommands::FitOnScreen.apply(&mut view, &target, None, h.ctx.pixels_per_point());
    let to_screen = |p: (f32, f32)| view.to_screen(craft::behavior::kurbo::Point::new(f64::from(p.0), f64::from(p.1)));
    let (a, b) = (to_screen(from), to_screen(to));
    h.hover_at(a);
    h.run_steps(1);
    h.drag_at(a);
    h.run_steps(1);
    for k in 1..=5 {
        h.hover_at(a + (b - a) * (k as f32 / 5.0));
        h.run_steps(1);
    }
    h.drop_at(b);
    h.run_steps(3);
    settle_scan(h)
}

/// A transparent signature in the ink's colour: no paper, table or shadow is left.
fn assert_clean_signature(image: &craft::review::SignatureImage) {
    let (w, h) = (image.width() as usize, image.height() as usize);
    assert!(w > 200 && w > h * 2, "both words are framed and trimmed: {w}×{h}");
    let rgba = image.rgba();
    let opaque: Vec<&[u8; 4]> = rgba.as_chunks::<4>().0.iter().filter(|p| p[3] > 128).collect();
    assert!(!opaque.is_empty());
    assert!(opaque.iter().all(|p| p[2] > p[0]), "strokes keep their blue ink");
    assert!(rgba.as_chunks::<4>().0.iter().filter(|p| p[3] == 0).count() > rgba.len() / 8, "the paper is transparent");
}

#[test]
fn scanned_signatures_are_framed_cleaned_adjusted_placed_and_remembered() {
    let _gpu = gpu();
    let path = signature_photo("scan");
    let files = Arc::new(ScriptedFiles::new());
    let mut h = harness_with_files(&files);
    h.execute("sign.fill.signature");
    h.run_steps(2);
    h.get_by_label("Scan").click();
    h.run_steps(2);
    assert!(!can_apply(&h), "Apply needs a photo");
    assert!(choose_photo(&mut h, &files, &path), "the photo is scanned and the cleaned signature is ready");
    if let Ok(dir) = std::env::var("PDFCRAFT_SHOTS") {
        h.render().unwrap().save(format!("{dir}/scan-signature-dialog.png")).unwrap();
    }

    // Drawing a frame around the first word only leaves the second out.
    assert!(drag_frame(&mut h, (110.0, 190.0), (400.0, 370.0)), "the result is ready after the drag");
    h.get_by_label("Apply").click();
    h.run_steps(3);
    let Some(SavedSig::Image(one_word)) = h.state().signature.clone() else { panic!("the scan is saved as an image signature") };
    assert!(one_word.width() < 300, "only the first word is left: {}", one_word.width());

    // Change it: the photo again, turned and turned back, then a frame with nothing in it.
    h.execute("sign.fill.signature.change");
    h.run_steps(2);
    h.get_by_label("Scan").click();
    h.run_steps(2);
    assert!(choose_photo(&mut h, &files, &path));
    h.get_by_label("Rotate clockwise").click();
    assert!(settle_scan(&mut h), "a turned photo is scanned again");
    h.get_by_label("Rotate counterclockwise").click();
    assert!(settle_scan(&mut h), "turned back, the scan is ready again");
    // A photo with nothing in the frame says so instead of offering an empty signature.
    assert!(!drag_frame(&mut h, (650.0, 40.0), (770.0, 140.0)), "an empty frame offers nothing to apply");
    h.get_by_label("No signature was found in the frame. Move the frame over the signature or raise the ink sensitivity.");
    // The frame around both words.
    assert!(drag_frame(&mut h, (100.0, 180.0), (700.0, 420.0)));

    h.get_by_label("Apply").click();
    h.run_steps(3);
    let Some(SavedSig::Image(saved)) = h.state().signature.clone() else { panic!("the scan is saved as an image signature") };
    assert_clean_signature(&saved);
    click(&mut h, 40.0, 250.0);
    assert_eq!(items(&h).len(), 1);
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().info.annotations[0].subtype, "Stamp");

    // It is remembered like any image signature, without the photo or its path.
    let settings = serde_json::to_string(&h.state().settings_snapshot()).unwrap();
    assert!(!settings.contains(&path.to_string_lossy().to_string()));
    assert!(settings.len() < 200_000, "only the trimmed signature is kept: {} bytes", settings.len());
    let mut again = crate::control::new_app();
    again.set_option(&egui::Context::default(), "language", "en").unwrap();
    restore_from(&mut again, h.state());
    assert_eq!(again.signature, h.state().signature);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn scan_cancel_errors_and_stale_answers_preserve_saved_signatures() {
    let path = signature_photo("scan-cancel");
    let files = Arc::new(ScriptedFiles::held());
    let mut h = harness_with_files(&files);
    let original = SavedSig::Typed("Ada Lovelace".into());
    h.state_mut().signature = Some(original.clone());
    h.execute("sign.fill.signature.change");
    h.run_steps(2);
    h.get_by_label("Scan").click();
    h.run_steps(2);
    files.queue(Picked::Cancelled);
    h.get_by_label("Choose photo…").click();
    h.run_steps(3);
    assert!(!can_apply(&h), "a cancelled pick loads nothing");
    assert!(choose_photo(&mut h, &files, &path));
    let broken = b"\xFF\xD8\xFF not a JPEG".to_vec();
    files.queue(Picked::Contents(vec![("broken.jpg".into(), broken)]));
    h.get_by_label("Choose photo…").click();
    h.run_steps(3);
    assert!(settle_scan(&mut h), "an invalid photo keeps the previous one");
    assert!(h.query_all_by_label_contains("Couldn't use the photo").next().is_some(), "the pad says why");
    // A photo picked for the Scan tab does not land in the Image tab.
    h.get_by_label("Choose photo…").click();
    h.run_steps(1);
    assert_eq!(files.open(), 1, "the photo pick waits for its answer");
    h.get_by_label("Image").click();
    h.run_steps(2);
    files.answer(Picked::Contents(vec![("photo".into(), std::fs::read(&path).unwrap())]));
    h.run_steps(3);
    assert!(h.query_by_label("Browse…").is_some(), "the Image tab is showing");
    h.get_by_label("Clear").click();
    h.run_steps(2);
    assert!(!can_apply(&h), "the photo did not become the Image tab's picture");
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert_eq!(h.state().signature, Some(original));
    assert!(items(&h).is_empty());
    std::fs::remove_file(path).unwrap();
}

/// A file dropped on the window, as the windowing layer hands it over.
#[derive(Debug)]
struct Dropped(std::path::PathBuf, Vec<u8>);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &std::path::Path {
        &self.0
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        Ok(self.1.clone())
    }
}

#[test]
fn photos_and_images_dropped_on_the_signature_pad_are_imported_not_opened() {
    let photo = signature_photo("drop");
    let mut h = harness();
    let tabs = h.state().views.len();
    h.execute("sign.fill.signature");
    h.run_steps(2);
    h.get_by_label("Scan").click();
    h.run_steps(2);
    // From a file manager (an absolute path) and from a browser-like source (bytes only).
    for file in [Dropped(photo.clone(), Vec::new()), Dropped("photo.png".into(), std::fs::read(&photo).unwrap())] {
        h.get_by_label("Clear").scroll_to_me();
        h.run_steps(2);
        h.get_by_label("Clear").click();
        h.run_steps(2);
        assert!(!can_apply(&h));
        h.input_mut().dropped_files.push(std::sync::Arc::new(file));
        h.run_steps(3);
        assert!(settle_scan(&mut h), "the photo is scanned");
    }
    h.get_by_label("Image").click();
    h.run_steps(2);
    h.get_by_label("Clear").click();
    h.run_steps(2);
    assert!(!can_apply(&h));
    h.input_mut().dropped_files.push(std::sync::Arc::new(Dropped("photo.png".into(), std::fs::read(&photo).unwrap())));
    settle_decode(&mut h);
    assert!(can_apply(&h), "the Image tab takes the picture as it is");
    h.input_mut().dropped_files.push(std::sync::Arc::new(Dropped("broken.png".into(), b"broken".to_vec())));
    settle_decode(&mut h);
    assert!(can_apply(&h), "an invalid file keeps the previous draft");
    assert!(shows(&h, "That image could not be read."), "the pad says why");
    assert_eq!(h.state().views.len(), tabs, "nothing opened behind the dialog");
    std::fs::remove_file(photo).unwrap();
}

/// #2149: the first save after Fill & Sign places a signature asks whether to lock it into the
/// page. Lock and save flattens it; Keep editable keeps it a comment; Don't show again becomes
/// the preference; the Fill & Sign panel has the switch too, once, and the Sign menu no longer
/// does (review on #2348).
#[test]
fn the_first_save_after_placing_a_signature_asks_to_lock_it() {
    let out = std::env::temp_dir().join(format!("pdfcraft-lock-signature-{}.pdf", std::process::id()));
    let files = Arc::new(ScriptedFiles::new());
    let mut h = harness_saving(&files, &out);
    h.state_mut().signature = Some(SavedSig::Typed("Ada Lovelace".into()));
    h.execute("sign.fill.signature");
    h.run_steps(2);
    click(&mut h, 40.0, 300.0);
    let signed = |h: &Harness<'static, PdfCraftApp>| {
        h.state().session.get(h.state().views[0].id).unwrap().info.annotations.iter().filter(|a| a.fill_sign).count()
    };
    assert_eq!(signed(&h), 1);
    // Keep editable: saved, still a comment, and not asked again for this placement.
    let ctx_h = h.ctx.clone();
    assert!(!h.state_mut().save_active(&ctx_h, SaveTarget::InPlace), "asks first");
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::Dialog, "Lock your signature into the page?");
    h.get_by_label("Keep editable").click();
    h.run_steps(3);
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty, "saved");
    assert_eq!(signed(&h), 1, "still a removable comment");
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().save_active(&ctx_h, SaveTarget::InPlace), "no new signature: no question");
    // A new signature asks again; Lock and save with Don't show again flattens and remembers.
    h.execute("sign.fill.signature");
    h.run_steps(2);
    click(&mut h, 40.0, 200.0);
    let ctx_h = h.ctx.clone();
    assert!(!h.state_mut().save_active(&ctx_h, SaveTarget::InPlace));
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::CheckBox, "Don't show again").click();
    h.run_steps(1);
    h.get_by_label("Lock and save").click();
    h.run_steps(3);
    assert_eq!(signed(&h), 0, "locked into the page");
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty);
    assert!(h.state().flatten_fill_sign_on_save, "remembered as the preference");
    // The switch is in the panel's Sign yourself section, once.
    h.state_mut().flatten_fill_sign_on_save = false;
    h.run_steps(2);
    assert!(h.query_by_label("Lock into the page when saving").is_none(), "the panel is closed");
    h.state_mut().left = pdfcraft_ui_egui::LeftPanel::Tool("fill_sign");
    h.state_mut().set_left_open(true);
    h.run_steps(3);
    h.get_by_label("Use signature");
    assert_eq!(h.query_all_by_label("Lock into the page when saving").count(), 1, "once, in the panel");
    h.get_by_label("Lock into the page when saving").click();
    h.run_steps(2);
    assert!(h.state().flatten_fill_sign_on_save);
    let _ = std::fs::remove_file(&out);
}

/// #2348 review (CodeRabbit): answering the lock question doesn't forget the signature; only a
/// save that succeeds does. When the save after the answer fails, the next save asks again.
#[test]
fn a_save_that_fails_after_the_lock_question_asks_again() {
    let bad = std::path::PathBuf::from("/pdfcraft-no-such-dir/out.pdf");
    let good = std::path::PathBuf::from("/pdfcraft-lock-retry.pdf");
    let files = Arc::new(ScriptedFiles::new().failing_writes(&bad));
    let mut h = harness_saving(&files, &bad);
    h.state_mut().signature = Some(SavedSig::Typed("Ada Lovelace".into()));
    h.execute("sign.fill.signature");
    h.run_steps(2);
    click(&mut h, 40.0, 300.0);
    let ctx_h = h.ctx.clone();
    assert!(!h.state_mut().save_active(&ctx_h, SaveTarget::InPlace), "asks first");
    h.run_steps(2);
    h.get_by_label("Keep editable").click();
    h.run_steps(3);
    assert!(h.state().session.get(h.state().views[0].id).unwrap().dirty, "the save failed");
    // The save after the answer doesn't ask twice in a row, but the failed one left the signature
    // unsaved: the next save asks again.
    // The refused file stays refused, so the retry is a Save As, answered with a file that writes.
    files.queue(Picked::Paths(vec![good.clone()]));
    let ctx_h = h.ctx.clone();
    assert!(!h.state_mut().save_active(&ctx_h, SaveTarget::As), "asks again");
    h.run_steps(2);
    h.get_by_label("Keep editable").click();
    h.run_steps(3);
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty, "saved this time");
    assert!(files.file(&good).is_some(), "the retry wrote the file");
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().save_active(&ctx_h, SaveTarget::InPlace), "saved: no question");
}

/// #2348 review (CodeRabbit): File ▸ Revert discards a signature just placed, so the save after it
/// doesn't ask to lock one, even when the saved file holds an earlier Fill & Sign signature.
#[test]
fn reverting_forgets_a_signature_just_placed() {
    let out = std::env::temp_dir().join(format!("pdfcraft-lock-revert-{}.pdf", std::process::id()));
    let files = Arc::new(ScriptedFiles::new());
    let mut h = harness_saving(&files, &out);
    h.state_mut().signature = Some(SavedSig::Typed("Ada Lovelace".into()));
    // An earlier signature, saved as a comment: the file keeps a Fill & Sign mark.
    h.execute("sign.fill.signature");
    h.run_steps(2);
    click(&mut h, 40.0, 300.0);
    let ctx_h = h.ctx.clone();
    assert!(!h.state_mut().save_active(&ctx_h, SaveTarget::InPlace), "asks first");
    h.run_steps(2);
    h.get_by_label("Keep editable").click();
    h.run_steps(3);
    // A new signature, then Revert: the new one is gone, the saved one stays.
    h.execute("sign.fill.signature");
    h.run_steps(2);
    click(&mut h, 40.0, 200.0);
    let ctx_h = h.ctx.clone();
    h.state_mut().revert_active(&ctx_h);
    h.run_steps(3);
    let marks = h.state().session.get(h.state().views[0].id).unwrap().info.annotations.iter().filter(|a| a.fill_sign).count();
    assert_eq!(marks, 1, "the saved signature is back, the new one is gone");
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().save_active(&ctx_h, SaveTarget::InPlace), "no signature was placed since the last save: no question");
    let _ = std::fs::remove_file(&out);
}

/// #2348 review (benandez): right after a signature is placed, a notice offers to lock it into
/// the page now. Lock now flattens it (undoable) without saving, so the save that follows doesn't
/// ask; undoing the lock brings back both the notice and the question. Dismiss hides the notice
/// until the next signature. A digitally signed document, or one whose saves lock anyway, gets no
/// notice.
#[test]
fn placing_a_signature_offers_to_lock_it_now() {
    const NOTICE: &str = "Signature added. Others can move it until it's locked.";
    let out = std::env::temp_dir().join(format!("pdfcraft-lock-now-{}.pdf", std::process::id()));
    let place = |h: &mut Harness<'static, PdfCraftApp>, y: f32| {
        h.state_mut().signature = Some(SavedSig::Typed("Ada Lovelace".into()));
        h.execute("sign.fill.signature");
        h.run_steps(2);
        click(h, 40.0, y);
        h.run_steps(2);
    };
    let marks = |h: &Harness<'static, PdfCraftApp>| {
        h.state().session.get(h.state().views[0].id).unwrap().info.annotations.iter().filter(|a| a.fill_sign).count()
    };
    let files = Arc::new(ScriptedFiles::new());
    let mut h = harness_saving(&files, &out);
    assert!(h.query_by_label(NOTICE).is_none(), "nothing placed yet");
    place(&mut h, 300.0);
    h.get_by_label(NOTICE);
    h.get_by_label("Lock now").click();
    h.run_steps(3);
    assert_eq!(marks(&h), 0, "locked into the page");
    assert!(h.query_by_label(NOTICE).is_none(), "nothing left to lock");
    assert!(h.state().session.get(h.state().views[0].id).unwrap().dirty, "locked, not saved");
    // Undo brings the movable signature back, with the notice and the save-time question.
    assert!(h.execute("edit.undo"));
    h.run_steps(3);
    assert_eq!(marks(&h), 1);
    h.get_by_label(NOTICE);
    let ctx_h = h.ctx.clone();
    assert!(!h.state_mut().save_active(&ctx_h, SaveTarget::InPlace), "asks again after undoing the lock");
    h.run_steps(2);
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    // Locked again, the save doesn't ask.
    h.get_by_label("Lock now").click();
    h.run_steps(3);
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().save_active(&ctx_h, SaveTarget::InPlace), "already locked: no question");
    // Dismiss hides it until the next signature.
    place(&mut h, 200.0);
    h.get_by_label(NOTICE).get_by_label("Dismiss").click();
    h.run_steps(2);
    assert!(h.query_by_label(NOTICE).is_none(), "dismissed");
    place(&mut h, 100.0);
    h.get_by_label(NOTICE);
    // Once saved, a signature kept as a comment is no longer just placed: no notice.
    let ctx_h = h.ctx.clone();
    assert!(!h.state_mut().save_active(&ctx_h, SaveTarget::InPlace), "asks first");
    h.run_steps(2);
    h.get_by_label("Keep editable").click();
    h.run_steps(3);
    assert!(marks(&h) > 0, "still a comment");
    assert!(h.query_by_label(NOTICE).is_none(), "saved: nothing just placed");
    place(&mut h, 50.0);
    h.get_by_label(NOTICE);
    // Saves that lock anyway need no offer.
    h.state_mut().flatten_fill_sign_on_save = true;
    h.run_steps(2);
    assert!(h.query_by_label(NOTICE).is_none(), "saving locks it anyway");
    let _ = std::fs::remove_file(&out);

    // Digitally signed: no notice, as locking would change the document after signing.
    let mut doc = {
        let mut h = harness();
        let app = h.state_mut();
        let id = app.views[0].id;
        let p12 = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12")).unwrap();
        let digital_id = pdfcraft_engine::sign::pkcs12::open(&p12, "test").unwrap();
        let opts = pdfcraft_engine::SignOptions { page: 0, rect: Some([20.0, 20.0, 160.0, 50.0]), ..Default::default() };
        app.session.sign(id, &digital_id, opts).expect("signs")
    };
    let mut h = harness_bytes(&std::mem::take(&mut doc));
    place(&mut h, 300.0);
    assert_eq!(marks(&h), 1);
    assert!(h.query_by_label(NOTICE).is_none(), "a signed document isn't offered the lock");
}

/// #2348 review: locking takes the placed signature out of the comments, so its selection goes
/// too; a stale (page, index) would point at nothing, or at another comment.
#[test]
fn locking_a_signature_clears_the_comment_selection() {
    let mut h = harness();
    h.state_mut().signature = Some(SavedSig::Typed("Ada Lovelace".into()));
    h.execute("sign.fill.signature");
    h.run_steps(2);
    click(&mut h, 40.0, 300.0);
    h.run_steps(2);
    assert!(h.state().views[0].session.comments.primary().is_some(), "the placed signature is selected");
    h.get_by_label("Lock now").click();
    h.run_steps(3);
    assert!(h.state().views[0].session.comments.primary().is_none(), "nothing left to select");
}

/// #2348 review: when locking the signature fails (here the document's security allows comments
/// but not changes), nothing is lost: the next save asks again, and "Don't show again" isn't
/// remembered.
#[test]
fn a_lock_that_fails_keeps_the_question() {
    let out = std::env::temp_dir().join(format!("pdfcraft-lock-fails-{}.pdf", std::process::id()));
    let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(FIXTURE.to_vec())).unwrap();
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: "",
        owner_password: "owner",
        // Everything but bit 4, modifying the document.
        permissions: -1 ^ 8,
        encrypt_metadata: true,
        seed: [4; 32],
    })
    .unwrap();
    let files = Arc::new(ScriptedFiles::new());
    let mut h =
        harness_on(&pdfcraft_cos::write_full(&doc, &Default::default()).unwrap(), Some(files.clone()), Some(out.to_string_lossy().into_owned()));
    h.state_mut().signature = Some(SavedSig::Typed("Ada Lovelace".into()));
    h.execute("sign.fill.signature");
    h.run_steps(2);
    click(&mut h, 40.0, 300.0);
    let signed = |h: &Harness<'static, PdfCraftApp>| {
        h.state().session.get(h.state().views[0].id).unwrap().info.annotations.iter().filter(|a| a.fill_sign).count()
    };
    assert_eq!(signed(&h), 1, "comments are allowed");
    let ctx_h = h.ctx.clone();
    assert!(!h.state_mut().save_active(&ctx_h, SaveTarget::InPlace), "asks first");
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::CheckBox, "Don't show again").click();
    h.run_steps(1);
    h.get_by_label("Lock and save").click();
    h.run_steps(3);
    assert_eq!(signed(&h), 1, "changes aren't allowed: still a comment");
    assert!(!h.state().flatten_fill_sign_on_save, "the failed answer isn't remembered");
    let ctx_h = h.ctx.clone();
    assert!(!h.state_mut().save_active(&ctx_h, SaveTarget::InPlace), "the next save asks again");
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::Dialog, "Lock your signature into the page?");
    let _ = std::fs::remove_file(&out);
}

/// #2348 review: the lock question stays out of the way where it must. Answering it on a file
/// that changed on disk leaves the changed-on-disk question showing (it was closed with the lock
/// question, so the save silently didn't happen); a signature placed and then undone asks
/// nothing; and a digitally signed document isn't asked, since locking would change it after
/// signing.
#[test]
fn the_lock_question_is_skipped_or_kept_where_it_must_be() {
    let place = |h: &mut Harness<'static, PdfCraftApp>| {
        h.state_mut().signature = Some(SavedSig::Typed("Ada Lovelace".into()));
        h.execute("sign.fill.signature");
        h.run_steps(2);
        click(h, 40.0, 300.0);
    };
    // A file that changed on disk.
    let path = std::env::temp_dir().join(format!("pdfcraft-lock-changed-{}.pdf", std::process::id()));
    std::fs::write(&path, FIXTURE).unwrap();
    let mut h = harness_bytes(FIXTURE);
    h.state_mut().close_tab(0);
    h.state_mut().open_path(&path.to_string_lossy());
    for _ in 0..100 {
        if !h.state().views.is_empty() {
            break;
        }
        h.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.run_steps(3);
    place(&mut h);
    std::fs::write(&path, [FIXTURE, b"\n% rewritten elsewhere\n".as_slice()].concat()).unwrap();
    let ctx_h = h.ctx.clone();
    assert!(!h.state_mut().save_active(&ctx_h, SaveTarget::InPlace), "asks first");
    h.run_steps(2);
    h.get_by_label("Keep editable").click();
    h.run_steps(3);
    assert!(!h.state().dialogs.is_empty(), "the changed-on-disk question shows");
    assert!(h.query_by_label("Lock your signature into the page?").is_none(), "in place of the lock question");
    let _ = std::fs::remove_file(&path);

    // Placed, then undone: nothing to lock, so the save just saves.
    let out = std::env::temp_dir().join(format!("pdfcraft-lock-undone-{}.pdf", std::process::id()));
    let files = Arc::new(ScriptedFiles::new());
    let mut h = harness_saving(&files, &out);
    place(&mut h);
    assert!(h.execute("edit.undo"));
    h.run_steps(2);
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().save_active(&ctx_h, SaveTarget::InPlace), "no question after the signature was undone");
    let _ = std::fs::remove_file(&out);

    // Digitally signed: never asked.
    let mut doc = {
        let mut h = harness();
        let app = h.state_mut();
        let id = app.views[0].id;
        let p12 = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12")).unwrap();
        let digital_id = pdfcraft_engine::sign::pkcs12::open(&p12, "test").unwrap();
        let opts = pdfcraft_engine::SignOptions { page: 0, rect: Some([20.0, 20.0, 160.0, 50.0]), ..Default::default() };
        app.session.sign(id, &digital_id, opts).expect("signs")
    };
    let mut h = harness_on(&std::mem::take(&mut doc), Some(files.clone()), Some(out.to_string_lossy().into_owned()));
    place(&mut h);
    let id = h.state().views[0].id;
    assert!(h.state().session.get(id).unwrap().is_signed());
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().save_active(&ctx_h, SaveTarget::InPlace), "a signed document isn't asked");
    let _ = std::fs::remove_file(&out);
}

/// #2149: Cancel saves nothing, and Keep editable with Don't show again turns the question off.
#[test]
fn cancelling_the_lock_question_saves_nothing() {
    let out = std::env::temp_dir().join(format!("pdfcraft-lock-cancel-{}.pdf", std::process::id()));
    let _ = std::fs::remove_file(&out);
    let files = Arc::new(ScriptedFiles::new());
    let mut h = harness_saving(&files, &out);
    h.state_mut().signature = Some(SavedSig::Typed("Ada Lovelace".into()));
    h.execute("sign.fill.signature");
    h.run_steps(2);
    click(&mut h, 40.0, 300.0);
    let ctx_h = h.ctx.clone();
    assert!(!h.state_mut().save_active(&ctx_h, SaveTarget::InPlace));
    h.run_steps(2);
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert!(files.file(&out).is_none() && h.state().session.get(h.state().views[0].id).unwrap().dirty, "nothing saved");
    let ctx_h = h.ctx.clone();
    assert!(!h.state_mut().save_active(&ctx_h, SaveTarget::InPlace), "asked again");
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::CheckBox, "Don't show again").click();
    h.run_steps(1);
    h.get_by_label("Keep editable").click();
    h.run_steps(3);
    assert!(files.file(&out).is_some() && !h.state().ask_to_lock_signatures);
    let restored = {
        let mut a = crate::control::new_app();
        restore_from(&mut a, h.state());
        a.ask_to_lock_signatures
    };
    assert!(!restored, "kept with the settings");
    let _ = std::fs::remove_file(&out);
}

/// Where a mode bar tab is, by its label.
fn mode_tab(h: &Harness<'static, PdfCraftApp>, label: &str) -> egui::Rect {
    h.get_by_role_and_label(egui::accesskit::Role::Tab, label).rect()
}

/// Scroll the page until `what` lies across every mode bar tab.
fn scroll_under_mode_tabs(h: &mut Harness<'static, PdfCraftApp>, what: impl Fn(&Harness<'static, PdfCraftApp>) -> egui::Rect) {
    let page = |h: &Harness<'static, PdfCraftApp>| h.state().views[0].page_screen_rect(0).expect("on screen");
    let y = what(h).center().y - page(h).top();
    let tabs = mode_tab(h, "Convert").center().y;
    h.state_mut().views[0].view.go_to_fraction(0, 0.0);
    h.run_steps(4);
    let top = page(h).top();
    let fraction = f64::from((top - (tabs - y)) / page(h).height());
    h.state_mut().views[0].view.go_to_fraction(0, fraction);
    h.run_steps(4);
    let under = what(h);
    for label in ["All tools", "Edit", "Convert", "E-Sign"] {
        let at = mode_tab(h, label).center();
        assert!(under.contains(at), "{under:?} lies under {label} at {at:?}");
    }
}

/// The Fill & Sign text box scrolled under the mode bar must not take its tabs' clicks (#2755).
#[test]
fn mode_tabs_keep_the_hand_over_a_type_box_scrolled_under_them() {
    let mut h = harness();
    assert!(h.execute("sign.fill.text"));
    h.set_option("left", "closed").unwrap();
    // The page across the window, so the box reaches under the tabs.
    h.state_mut().views[0].view.fit = pdfcraft_ui_egui::canvas::Fit::Width;
    h.run_steps(4);
    assert_eq!(h.state().quick_tool, QuickTool::Fill(FillTool::Text));
    click(&mut h, 10.0, 360.0);
    h.event(egui::Event::Text("Ada Lovelace, Countess of Lovelace".into()));
    h.run_steps(3);
    scroll_under_mode_tabs(&mut h, type_box_rect);
    let convert = mode_tab(&h, "Convert").center();
    h.hover_at(convert);
    h.run_steps(1);
    h.drag_at(convert);
    h.run_steps(1);
    h.drop_at(convert);
    h.run_steps(3);
    assert_eq!(h.state().mode, pdfcraft_ui_egui::Mode::Convert, "the tab takes the click, not the text box");
}

fn place_selected_text(h: &mut Harness<'static, PdfCraftApp>, text: &str) {
    h.execute("sign.fill.text");
    click(h, 50.0, 250.0);
    h.event(egui::Event::Text(text.into()));
    h.run_steps(1);
    commit_typed_text(h);
    let selected = {
        let s = h.state();
        s.session
            .get(s.views[0].id)
            .unwrap()
            .info
            .annotations
            .iter()
            .find(|a| a.contents.as_deref() == Some(text))
            .map(|a| (a.page, a.index))
            .unwrap()
    };
    h.state_mut().views[0].session.comments.select_only(selected);
    h.run_steps(2);
}

fn placed_height(h: &Harness<'static, PdfCraftApp>, text: &str) -> f32 {
    let s = h.state();
    let a = s.session.get(s.views[0].id).unwrap().info.annotations.iter().find(|a| a.contents.as_deref() == Some(text)).unwrap();
    a.rect[3] - a.rect[1]
}

fn placed_color(h: &Harness<'static, PdfCraftApp>, text: &str) -> Option<[f32; 3]> {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().info.annotations.iter().find(|a| a.contents.as_deref() == Some(text)).unwrap().color
}

/// The Fill & Sign bar's controls, one row each: a control is pressed by its label, then the tool
/// or the selected text changes.
#[test]
fn fill_and_sign_bar_controls_pick_tools_and_restyle_the_selection() {
    type Row = fn(&mut Harness<'static, PdfCraftApp>);
    // The size flyout opens from its icon and leaves a stored size alone.
    let size_flyout: Row = |h| {
        h.state_mut().fill_text_size = 9.1;
        place_selected_text(h, "Keep me");
        let height = placed_height(h, "Keep me");
        let holds_size = |h: &Harness<'static, PdfCraftApp>| h.query_all_by_role_and_label(egui::accesskit::Role::SpinButton, "Font size").count();
        assert_eq!(holds_size(h), 0);
        h.get_by_label("Font size").click();
        h.run_steps(4);
        assert!(holds_size(h) > 0, "the flyout holds the size");
        assert_eq!(h.state().fill_text_size, 9.1, "opening the flyout does not round the size");
        assert_eq!(placed_height(h, "Keep me"), height, "opening the flyout does not resize the selected text");
    };
    // The colour control recolours the selected text, as one undo step.
    let colour: Row = |h| {
        place_selected_text(h, "Paint me");
        let before = placed_color(h, "Paint me");
        h.get_by_label("Colour").click();
        h.run_steps(2);
        h.get_by_label("Blue").click();
        h.run_steps(3);
        let color = h.state().fill_text_color.map(|v| v as f32);
        assert!(color[2] > color[0], "blue was selected: {color:?}");
        assert_ne!(before, Some(color));
        assert_eq!(placed_color(h, "Paint me"), Some(color));
        h.execute("edit.undo");
        h.run_steps(3);
        assert_eq!(placed_color(h, "Paint me"), before, "one undo takes the colour back");
    };
    // The fill button shows the tool it picks.
    let shows_picked_tool: Row = |h| {
        fill_slot(h, "Fill & Sign: sign").click();
        h.run_steps(2);
        assert_eq!(h.state().quick_tool, QuickTool::Fill(FillTool::Signature));
        assert!(pad_open(h), "an empty slot opens the pad");
        h.get_by_label("Cancel").click();
        h.run_steps(3);
        assert!(!pad_open(h), "Cancel closes the pad");
        h.state_mut().signature = Some(SavedSig::Typed("Ada Lovelace".into()));
        h.state_mut().quick_tool = QuickTool::Select;
        h.run_steps(2);
        fill_slot(h, "Fill & Sign: sign").click();
        h.run_steps(2);
        assert_eq!(h.state().quick_tool, QuickTool::Fill(FillTool::Signature));
        assert!(!pad_open(h), "a saved signature opens no pad");
        h.state_mut().left = pdfcraft_ui_egui::LeftPanel::Tool("fill_sign");
        h.state_mut().set_left_open(true);
        h.run_steps(3);
        assert_eq!(h.get_by_label("Use signature").accesskit_node().is_selected(), Some(true), "the active tool's card is marked");
        h.execute("sign.fill.check");
        h.state_mut().quick_tool = QuickTool::Select;
        h.run_steps(2);
        fill_slot(h, "Fill & Sign: checkmark").click();
        h.run_steps(2);
        assert_eq!(h.state().quick_tool, QuickTool::Fill(FillTool::Check), "the button keeps the last Fill & Sign tool");
    };
    // A fill tool restored from full screen shows without replacing the last selection.
    let restored_tool: Row = |h| {
        h.execute("sign.fill.date");
        h.run_steps(2);
        h.state_mut().set_full_screen(true);
        h.run_steps(2);
        h.execute("sign.fill.check");
        h.run_steps(2);
        h.state_mut().set_full_screen(false);
        h.run_steps(2);
        assert_eq!(h.state().quick_tool, QuickTool::Fill(FillTool::Date));
        fill_slot(h, "Fill & Sign: date");
        h.state_mut().quick_tool = QuickTool::Select;
        h.run_steps(2);
        fill_slot(h, "Fill & Sign: checkmark").click();
        h.run_steps(2);
        assert_eq!(h.state().quick_tool, QuickTool::Fill(FillTool::Check));
    };
    // The fill button keeps Date after deselecting and across documents.
    let keeps_date: Row = |h| {
        fill_slot(h, "Fill & Sign: sign").click_secondary();
        h.run_steps(2);
        fill_flyout_row(h, "Fill  Sign: date").click();
        h.run_steps(2);
        assert_eq!(h.state().quick_tool, QuickTool::Fill(FillTool::Date));
        h.key_press(egui::Key::V);
        h.run_steps(2);
        assert_eq!(h.state().quick_tool, QuickTool::Select);
        let ctx_h = h.ctx.clone();
        h.state_mut().open_bytes(&ctx_h, "second.pdf", None, FIXTURE.to_vec()).unwrap();
        h.state_mut().active = Some(1);
        h.run_steps(2);
        fill_slot(h, "Fill & Sign: date").click();
        h.run_steps(2);
        assert_eq!(h.state().quick_tool, QuickTool::Fill(FillTool::Date), "a click picks the shown tool, not the default");
        assert!(!pad_open(h), "the default tool's dialog stays closed");
        h.key_press(egui::Key::V);
        h.run_steps(2);
        h.state_mut().close_tab(1);
        h.state_mut().close_tab(0);
        h.run_steps(2);
        let ctx_h = h.ctx.clone();
        h.state_mut().open_bytes(&ctx_h, "third.pdf", None, FIXTURE.to_vec()).unwrap();
        h.state_mut().active = Some(0);
        h.run_steps(2);
        fill_slot(h, "Fill & Sign: date").click();
        h.run_steps(2);
        assert_eq!(h.state().quick_tool, QuickTool::Fill(FillTool::Date), "the tool outlives closing every document");
    };
    for row in [size_flyout, colour, shows_picked_tool, restored_tool, keeps_date] {
        let mut h = harness();
        row(&mut h);
    }
}

#[test]
fn typing_a_fill_text_size_that_rounds_to_the_stored_one_adds_no_undo_step() {
    let mut h = harness();
    place_selected_text(&mut h, "Round me");
    let height = placed_height(&h, "Round me");
    h.get_by_label("Font size").click();
    h.run_steps(2);
    let type_size = |h: &mut Harness<'static, PdfCraftApp>, value: &str| {
        h.get_by_role_and_label(egui::accesskit::Role::SpinButton, "Font size").click();
        h.run_steps(2);
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        h.event(egui::Event::Text(value.into()));
        h.run_steps(1);
        h.key_press(egui::Key::Enter);
        h.run_steps(4);
    };
    type_size(&mut h, "36.1234");
    let resized = placed_height(&h, "Round me");
    assert!(resized > height, "the first size applies: {height} -> {resized}");
    type_size(&mut h, "36.1231");
    assert_eq!(h.state().fill_text_size, 36.1231);
    h.execute("edit.undo");
    h.run_steps(3);
    assert_eq!(placed_height(&h, "Round me"), height, "one undo takes back the only change; the rounded retype added no step");
}

#[test]
fn a_fill_tool_selected_while_the_quick_bar_is_hidden_is_remembered() {
    let mut h = harness();
    let mode = h.state().mode;
    h.set_option("mode", "read").unwrap();
    h.run_steps(2);
    h.set_option("quick", "fill-date").unwrap();
    h.run_steps(2);
    h.set_option("quick", "select").unwrap();
    h.state_mut().mode = mode;
    h.run_steps(2);
    fill_slot(&h, "Fill & Sign: date");

    h.state_mut().initials = Some(SavedSig::Typed("AL".into()));
    h.set_option("mode", "read").unwrap();
    h.run_steps(2);
    h.execute("sign.fill.initials.change");
    h.run_steps(2);
    h.get_by_label("Apply").click();
    h.run_steps(3);
    assert_eq!(h.state().quick_tool, QuickTool::Fill(FillTool::Initials));
    h.state_mut().quick_tool = QuickTool::Select;
    h.state_mut().mode = mode;
    h.run_steps(2);
    fill_slot(&h, "Fill & Sign: initials");
}

fn commit_typed_text(h: &mut Harness<'static, PdfCraftApp>) {
    let p = pos2(350.0, 850.0); // Blank canvas outside the page and the type box.
    h.hover_at(p);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(3);
}

fn fill_input(h: &mut Harness<'static, PdfCraftApp>, events: impl IntoIterator<Item = egui::Event>) {
    h.input_mut().events.extend(events);
    let ctx = h.ctx.clone();
    let mut raw = std::mem::take(h.input_mut());
    eframe::App::raw_input_hook(h.state_mut(), &ctx, &mut raw);
    *h.input_mut() = raw;
    h.step();
    h.run_steps(2);
}

fn fill_key(h: &mut Harness<'static, PdfCraftApp>, key: egui::Key) {
    fill_input(
        h,
        [true, false].map(|pressed| egui::Event::Key { key, physical_key: Some(key), pressed, repeat: false, modifiers: egui::Modifiers::NONE }),
    );
}

#[test]
fn fill_sign_enter_adds_lines_outside_click_commits_and_escape_cancels() {
    let shots = std::env::var_os("PDFCRAFT_FILL_SIGN_SHOTS").is_some();
    let _gpu = shots.then(gpu);
    let mut h = harness();
    assert!(h.execute("sign.fill.text"));
    click(&mut h, 50.0, 300.0);
    let initial_height = type_box_rect(&h).height();
    fill_key(&mut h, egui::Key::Enter);
    assert_eq!(
        h.state().views[0].session.fill_text.as_ref().map(|t| t.text.as_str()),
        Some("\n"),
        "Enter inserts a newline and keeps the editor open"
    );
    fill_input(&mut h, [egui::Event::Text("Ada".into())]);
    fill_key(&mut h, egui::Key::Enter);
    fill_key(&mut h, egui::Key::Enter);
    fill_input(&mut h, [egui::Event::Text("Lovelace".into())]);
    fill_key(&mut h, egui::Key::Enter);
    let text = "\nAda\n\nLovelace\n";
    assert_eq!(h.state().views[0].session.fill_text.as_ref().unwrap().text, text);
    assert!(items(&h).is_empty(), "Enter never places an annotation");
    // The page editor paints each line separately, including the empty lines' space.
    fn text_shape<'a>(shape: &'a egui::Shape, text: &str) -> Option<&'a egui::epaint::TextShape> {
        match shape {
            egui::Shape::Text(t) if t.galley.text() == text => Some(t),
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|s| text_shape(s, text)),
            _ => None,
        }
    }
    let line = |text| h.output().shapes.iter().find_map(|s| text_shape(&s.shape, text)).unwrap();
    let (ada, lovelace) = (line("Ada"), line("Lovelace"));
    assert_eq!(ada.galley.rows.len(), 1, "short typed lines fit without accidental soft wraps");
    assert_eq!(lovelace.galley.rows.len(), 1, "short typed lines fit without accidental soft wraps");
    assert!(lovelace.pos.y > ada.pos.y + ada.galley.size().y, "the empty line keeps its space");
    assert!(type_box_rect(&h).height() > initial_height * 3.0, "the editor grows with its lines");
    if shots {
        h.render().unwrap().save("/tmp/5850-multiline.png").unwrap();
    }
    let page = h.state().views[0].page_screen_rect(0).expect("on screen");
    let outside = pos2(page.left() - 20.0, page.center().y);
    h.hover_at(outside);
    h.drag_at(outside);
    h.run_steps(1);
    h.drop_at(outside);
    h.run_steps(3);
    assert!(h.state().views[0].session.fill_text.is_none());
    assert_eq!(items(&h), [("FreeText".to_owned(), Some(text.to_owned()))]);
    let id = h.state().views[0].id;
    let saved = h.state().session.save_bytes(id).unwrap();
    let mut session = pdfcraft_engine::Session::new();
    let reopened = session.open("again.pdf", None, saved.clone(), None).unwrap();
    let annot = &session.get(reopened).unwrap().info.annotations[0];
    assert_eq!(annot.contents.as_deref(), Some(text));
    assert!(annot.rect[3] - annot.rect[1] >= 5.0 * 10.0 * 1.2, "the saved box keeps all five lines");
    // Compared as edits, not saved bytes: a save stamps its time, so two saves a second apart differ.
    let unchanged = |h: &Harness<'static, PdfCraftApp>| {
        let doc = h.state().session.get(id).unwrap();
        (doc.edit_generation(), doc.dirty, doc.can_undo().map(str::to_owned))
    };
    let before = unchanged(&h);
    click(&mut h, 50.0, 180.0);
    fill_input(&mut h, [egui::Event::Text("Cancel me".into())]);
    fill_key(&mut h, egui::Key::Enter);
    fill_key(&mut h, egui::Key::Escape);
    assert!(h.state().views[0].session.fill_text.is_none());
    assert_eq!(unchanged(&h), before, "Escape cancels without writing");
    assert_eq!(items(&h), [("FreeText".to_owned(), Some(text.to_owned()))]);
}
