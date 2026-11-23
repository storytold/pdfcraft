//! Commenting in the real shell (egui_kittest): tools, gestures, selection, the composer and
//! the Comments panel.

use craft::host::{AppInfo, Host, Picked, ScriptedFiles};
use craft::review::{CommentSort, ReviewStatus};
use craft::ui::patterns::form_dialogs::{FieldValue, HostedParamDialog};
use egui::accesskit::Role;
use egui::{Pos2, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use pdfcraft_render::Annotation;
use pdfcraft_ui_egui::{PdfCraftApp, QuickTool};

use crate::support;
use crate::support::AppHarness as _;

/// Two 300×200 pages of Helvetica text.
const TEXT_FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 5 0 R /Resources << /Font << /F1 7 0 R >> >> >> endobj
4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 6 0 R /Resources << /Font << /F1 7 0 R >> >> >> endobj
5 0 obj << /Length 55 >> stream
BT /F1 14 Tf 20 150 Td (The quick brown fox) Tj ET
endstream endobj
6 0 obj << /Length 58 >> stream
BT /F1 14 Tf 20 150 Td (A second brown animal) Tj ET
endstream endobj
7 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj
trailer << /Root 1 0 R >>
%%EOF";

fn harness(setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    harness_with_bytes(TEXT_FIXTURE.to_vec(), setup)
}

fn visible_page_harness(setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    harness(|app| {
        // Keep the whole small fixture visible below the window's chrome.
        app.set_option(&egui::Context::default(), "zoom", "150").unwrap();
        setup(app);
    })
}

fn harness_with_bytes(bytes: Vec<u8>, setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    harness_with_size(bytes, egui::vec2(1400.0, 900.0), setup)
}

fn harness_with_size(bytes: Vec<u8>, size: egui::Vec2, setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(size).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "text.pdf", None, bytes).expect("opens");
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "author", "Tester").unwrap();
            setup(&mut app);
            app
        }
    });
    h.run_steps(4);
    crate::support::settle(&mut h);
    h
}

/// Like [`harness`], with the clipboard the app copies to, which the test reads back.
fn harness_copying(setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> (Harness<'static, PdfCraftApp>, support::RecordingClipboard) {
    let clipboard = support::RecordingClipboard::default();
    let kept = clipboard.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::support::app_with_clipboard(clipboard);
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "text.pdf", None, TEXT_FIXTURE.to_vec()).expect("opens");
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "author", "Tester").unwrap();
            setup(&mut app);
            app
        }
    });
    h.run_steps(4);
    crate::support::settle(&mut h);
    (h, kept)
}

/// Like [`harness`], on a host whose file picker answers from `files`.
fn harness_on_files(files: std::sync::Arc<ScriptedFiles>, setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let info = AppInfo { id: "pdfcraft", name: "PdfCraft", version: env!("CARGO_PKG_VERSION") };
            let host = Host::builder(info).files(files).build();
            let mut app = PdfCraftApp::with_host(std::sync::Arc::new(host), crate::control::services());
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "text.pdf", None, TEXT_FIXTURE.to_vec()).expect("opens");
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "author", "Tester").unwrap();
            setup(&mut app);
            app
        }
    });
    h.run_steps(4);
    crate::support::settle(&mut h);
    h
}

fn undo(h: &mut Harness<'static, PdfCraftApp>) {
    let ctx = h.ctx.clone();
    h.state_mut().undo(&ctx);
}

fn redo(h: &mut Harness<'static, PdfCraftApp>) {
    let ctx = h.ctx.clone();
    h.state_mut().redo(&ctx);
}

/// Opens Comment Properties for the comment at (`page`, `index`), with the harness's context.
fn open_comment_props(h: &mut Harness<'static, PdfCraftApp>, page: usize, index: usize) {
    let ctx = h.ctx.clone();
    h.state_mut().open_comment_props(&ctx, page, index);
}

/// A point on page 1 in PDF user space → screen (no rotation; MediaBox 300×200).
fn at(h: &Harness<'static, PdfCraftApp>, x: f32, y: f32) -> Pos2 {
    let r = h.state().views[0].page_screen_rect(0).expect("page 1 on screen");
    pos2(r.left() + x / 300.0 * r.width(), r.top() + (200.0 - y) / 200.0 * r.height())
}

fn drag(h: &mut Harness<'static, PdfCraftApp>, from: Pos2, to: Pos2) {
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(from + (to - from) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    h.drop_at(to);
    h.run_steps(3);
}

fn click(h: &mut Harness<'static, PdfCraftApp>, p: Pos2) {
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(3);
}

/// Drag between two page points (user space).
fn drag_pt(h: &mut Harness<'static, PdfCraftApp>, a: (f32, f32), b: (f32, f32)) {
    let (a, b) = (at(h, a.0, a.1), at(h, b.0, b.1));
    drag(h, a, b);
}

/// Click a page point (user space).
fn click_pt(h: &mut Harness<'static, PdfCraftApp>, p: (f32, f32)) {
    let p = at(h, p.0, p.1);
    click(h, p);
}
/// Click a page point with modifier keys held (⌘-click adds a comment to the selection).
fn click_pt_with(h: &mut Harness<'static, PdfCraftApp>, p: (f32, f32), modifiers: egui::Modifiers) {
    let p = at(h, p.0, p.1);
    h.hover_at(p);
    h.event(egui::Event::ModifiersChanged(modifiers));
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers });
        h.run_steps(1);
    }
    h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    h.run_steps(2);
}

/// A text field by its placeholder ("Add a comment", "Add a reply").
fn field<'a>(h: &'a Harness<'static, PdfCraftApp>, placeholder: &'a str) -> egui_kittest::Node<'a> {
    h.query_all_by(|n| n.placeholder() == Some(placeholder)).next().unwrap_or_else(|| panic!("no field {placeholder:?}"))
}

fn comments(h: &Harness<'static, PdfCraftApp>) -> Vec<Annotation> {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().info.annotations.clone()
}

/// #3418: choosing a sidebar comment must reveal the annotation, not just its page's top.
#[test]
fn sidebar_comment_centres_the_annotation_at_high_zoom() {
    for (layout, rotation) in [("continuous", 0), ("single", 0), ("two-page", 0), ("continuous", 90), ("continuous", 180), ("continuous", 270)] {
        let mut h = harness(move |app| {
            app.set_option(&egui::Context::default(), "panel", "comments").unwrap();
            app.set_option(&egui::Context::default(), "layout", layout).unwrap();
            app.set_option(&egui::Context::default(), "zoom", "800").unwrap();
            app.views[0].view.rotation = rotation;
            let shape = pdfcraft_engine::Shape::Note { at: [150.0, 90.0], icon: pdfcraft_engine::NoteIcon::Comment };
            assert!(app.apply_edit(
                &egui::Context::default(),
                pdfcraft_engine::Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
                    page: 1,
                    style: pdfcraft_engine::Style::default_for(&shape),
                    shape,
                    contents: "Reveal this note".into(),
                    author: "Tester".into(),
                })
            ));
            app.views[0].session.comments.clear_selection();
            app.set_option(&egui::Context::default(), "page", "1").unwrap();
        });
        // The panel names a card by its author, text and date.
        h.get_by_label_contains("Reveal this note").click();
        h.run_steps(8);
        assert_eq!(h.state().views[0].session.comments.selection(), vec![(1, 0)], "the sidebar card was selected");
        let a = &comments(&h)[0];
        let view = &h.state().views[0];
        let page = view.page_screen_rect(1).expect("the selected comment's page is drawn");
        let (u, v) = ((a.rect[0] + a.rect[2]) / 600.0, 1.0 - (a.rect[1] + a.rect[3]) / 400.0);
        let (u, v) = match rotation {
            90 => (1.0 - v, u),
            180 => (1.0 - u, 1.0 - v),
            270 => (v, 1.0 - u),
            _ => (u, v),
        };
        let centre = pos2(page.left() + u * page.width(), page.top() + v * page.height());
        let viewport = view.viewport_rect();
        assert!(centre.distance(viewport.center()) < 3.0, "{layout}, {rotation}°: comment at {centre:?}, viewport {viewport:?}");
        assert!((view.zoom() - 8.0).abs() < 0.01, "revealing a comment preserves zoom");
        if layout == "continuous"
            && rotation == 0
            && let Ok(dir) = std::env::var("PDFCRAFT_SHOTS")
        {
            let _gpu = crate::gpu();
            crate::support::settle(&mut h);
            h.render().unwrap().save(format!("{dir}/comment-centred.png")).unwrap();
        }
    }
}

#[test]
fn dragging_over_text_with_the_highlighter_highlights_it() {
    let mut h = harness(|app| app.set_option(&egui::Context::default(), "quick", "highlight").unwrap());
    let (a, b) = {
        let v = &h.state().views[0];
        (v.glyph_screen_pos(0, 4).expect("text layer"), v.glyph_screen_pos(0, 14).expect("glyph"))
    };
    drag(&mut h, a, b);
    let c = comments(&h);
    assert_eq!(c.len(), 1, "{c:?}");
    assert_eq!((c[0].subtype.as_str(), c[0].author.as_deref(), c[0].quads.len()), ("Highlight", Some("Tester"), 1));
    assert_eq!(h.state().views[0].session.comments.selection(), vec![(0, 0)], "the new comment is selected");
    assert_eq!(h.state().quick_tool, QuickTool::Comment(pdfcraft_ui_egui::comments::CommentTool::Highlight), "the highlighter stays on");
    // The Comments panel opened with the tool and lists it.
    h.get_by_role_and_label(Role::Button, "Comments");
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Add highlight"));
}

#[test]
fn the_quick_bar_colour_changes_the_selected_comment() {
    use pdfcraft_ui_egui::comments::CommentTool;

    let mut h = harness(|app| app.set_option(&egui::Context::default(), "quick", "highlight").unwrap());
    let (a, b) = {
        let v = &h.state().views[0];
        (v.glyph_screen_pos(0, 4).expect("text layer"), v.glyph_screen_pos(0, 14).expect("glyph"))
    };
    drag(&mut h, a, b);
    assert_eq!(comments(&h)[0].color, Some([1.0, 0.94, 0.0]));

    h.get_by_label("Colour").click();
    h.run_steps(2);
    h.get_by_label("Blue").click();
    h.run_steps(3);

    let state = h.state();
    let doc = state.session.get(state.views[0].id).expect("document");
    assert_eq!(doc.comment_props(0, 0).and_then(|props| props.color), Some([0.0, 0.47, 0.84]));
    assert_eq!(doc.can_undo(), Some("Change comment properties"), "recoloring is one undo step");
    assert_eq!(state.comment_prefs.style(CommentTool::Highlight).color, [0.0, 0.47, 0.84]);
}

#[test]
fn the_quick_bar_colour_changes_the_selected_arrow() {
    use pdfcraft_ui_egui::comments::CommentTool;

    // Arrows are saved as Line annotations; the Arrow tool's colour must still reach them.
    let mut h = visible_page_harness(|app| app.set_option(&egui::Context::default(), "quick", "arrow").unwrap());
    drag_pt(&mut h, (20.0, 20.0), (120.0, 30.0));
    assert_eq!(comments(&h)[0].subtype, "Line");
    assert_eq!(h.state().quick_tool, QuickTool::Comment(CommentTool::Arrow), "the arrow tool is still active");
    assert_eq!(h.state().views[0].session.comments.selection(), vec![(0, 0)], "the new arrow is selected");

    h.get_by_label("Colour").click();
    h.run_steps(2);
    h.get_by_label("Blue").click();
    h.run_steps(3);

    let state = h.state();
    let doc = state.session.get(state.views[0].id).expect("document");
    assert_eq!(doc.comment_props(0, 0).and_then(|props| props.color), Some([0.0, 0.47, 0.84]));
    assert_eq!(doc.can_undo(), Some("Change comment properties"));
}

#[test]
fn the_quick_bar_colour_skips_the_selected_comment_when_commenting_is_forbidden() {
    use pdfcraft_ui_egui::comments::CommentTool;

    // One page with a yellow highlight, encrypted to allow printing only (no commenting).
    let plain = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Annots [4 0 R] >> endobj
4 0 obj << /Type /Annot /Subtype /Highlight /Rect [20 140 120 165] /QuadPoints [20 165 120 165 20 140 120 140] /C [1 0.94 0] >> endobj
trailer << /Root 1 0 R >>
%%EOF";
    let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(plain.to_vec())).expect("fixture parses");
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: "",
        owner_password: "owner",
        permissions: 0b0100,
        encrypt_metadata: true,
        seed: [5; 32],
    })
    .expect("encrypts");
    let bytes = pdfcraft_cos::write_full(&doc, &Default::default()).expect("writes");
    let mut h = harness(move |app| {
        app.open_bytes(&egui::Context::default(), "restricted.pdf", None, bytes).expect("opens restricted");
        app.set_option(&egui::Context::default(), "quick", "highlight").unwrap();
    });
    let id = h.state().active_ids().expect("active document").1;
    assert!(!h.state().session.get(id).expect("document").allows_annotation(), "commenting is forbidden");
    let index = h.state().views.iter().position(|v| v.id == id).expect("view");
    h.state_mut().views[index].session.comments.select_only((0, 0));
    h.run_steps(2);

    h.get_by_label("Colour").click();
    h.run_steps(2);
    h.get_by_label("Blue").click();
    h.run_steps(3);

    let notice = support::shown_notice(&mut h);
    let state = h.state();
    let doc = state.session.get(id).expect("document");
    assert_eq!(doc.comment_props(0, 0).and_then(|props| props.color), Some([1.0, 0.94, 0.0]), "the comment keeps its colour");
    assert_eq!(doc.can_undo(), None, "no edit was made");
    assert_eq!(notice, None, "picking a colour is not reported as a failed edit");
    assert_eq!(state.comment_prefs.style(CommentTool::Highlight).color, [0.0, 0.47, 0.84], "the tool default still changes");
}

#[test]
fn a_new_highlight_opens_its_note_for_typing() {
    let mut h = harness(|app| {
        app.set_option(&egui::Context::default(), "panel", "comments").unwrap();
        app.set_option(&egui::Context::default(), "quick", "highlight").unwrap();
    });
    assert_eq!(note(&h), None);
    let (a, b) = {
        let v = &h.state().views[0];
        (v.glyph_screen_pos(0, 4).expect("text layer"), v.glyph_screen_pos(0, 14).expect("glyph"))
    };
    drag(&mut h, a, b);
    assert_eq!(comments(&h).len(), 1);
    assert_eq!(note(&h), Some(String::new()), "the Comments panel edits the new highlight");
    assert_eq!(h.state().views[0].session.comments.selection(), vec![(0, 0)]);
    // An area highlight opens its note too, replacing the empty one.
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    assert_eq!(comments(&h).len(), 2);
    assert_eq!(note(&h), Some(String::new()));
    assert_eq!(h.state().views[0].session.comments.selection(), vec![(0, 1)], "the editor belongs to the second highlight");
    // A note being typed is kept: the next highlight is only selected.
    h.event(egui::Event::Text("typed".into()));
    h.run_steps(2);
    drag_pt(&mut h, (160.0, 100.0), (260.0, 40.0));
    assert_eq!(comments(&h).len(), 3);
    assert_eq!(h.state().views[0].session.comments.primary(), Some((0, 2)));
    assert_eq!(note(&h).as_deref(), Some("typed"));
}

/// The Comments panel's editor for a comment's note (labelled "Edit", distinct from the panel's
/// top "Add a comment" field and each card's "Add a reply" field).
fn note_editor<'a>(h: &'a Harness<'static, PdfCraftApp>) -> egui_kittest::Node<'a> {
    h.query_all_by(|n| n.role() == egui::accesskit::Role::MultilineTextInput && !n.is_hidden() && n.label().as_deref() == Some("Edit"))
        .next()
        .expect("note editor")
}

/// The text in the open note editor.
fn note(h: &Harness<'static, PdfCraftApp>) -> Option<String> {
    h.state().views[0].session.comments.panel.editing.as_ref().map(|(_, _, text)| text.clone())
}

/// The open pop-up windows on screen whose comment is by `author`: `NotePopup`'s access nodes,
/// labelled with the title of the window's first message, the comment's kind and author
/// ("Note – Tester").
fn popup_windows<'a>(h: &'a Harness<'static, PdfCraftApp>, author: &str) -> Vec<egui_kittest::Node<'a>> {
    // The window is `NoteWindow`, a `Role::GROUP` container labelled with the first message's title.
    let tail = format!("– {author}");
    h.query_all_by(move |n| n.role() == Role::Group && n.label().is_some_and(|l| l.ends_with(&tail))).collect()
}

/// The window rect of the only pop-up of `author`'s comment, in screen points.
fn popup_rect(h: &Harness<'static, PdfCraftApp>, author: &str) -> egui::Rect {
    let windows = popup_windows(h, author);
    assert_eq!(windows.len(), 1, "one pop-up of {author}'s comment");
    windows[0].rect()
}

/// A letter key as a keyboard sends it: the key goes down, types its letter and comes up.
fn type_key(h: &mut Harness<'static, PdfCraftApp>, key: egui::Key, letter: &str) {
    h.key_down(key);
    h.event(egui::Event::Text(letter.into()));
    h.key_up(key);
    h.run_steps(2);
}

/// Highlight glyphs `from..to` of page 1's text with the highlighter.
fn highlight_text(h: &mut Harness<'static, PdfCraftApp>, from: usize, to: usize) {
    let (a, b) = {
        let v = &h.state().views[0];
        (v.glyph_screen_pos(0, from).expect("text layer"), v.glyph_screen_pos(0, to).expect("glyph"))
    };
    drag(h, a, b);
    h.run_steps(2);
}

#[test]
fn clicking_the_page_gives_the_keyboard_back_from_a_highlight_note() {
    // #2669: the note editor took the keyboard back on every frame, so after a click on the
    // page V and H were typed into the note instead of picking a tool.
    let mut h = harness(|app| {
        app.set_option(&egui::Context::default(), "quick", "highlight").unwrap();
        app.set_option(&egui::Context::default(), "panel", "comments").unwrap();
    });
    highlight_text(&mut h, 4, 14);
    assert!(note_editor(&h).is_focused(), "a new highlight's note takes the keyboard straight away");
    h.event(egui::Event::Text("Note".into()));
    h.run_steps(2);
    assert_eq!(note(&h).as_deref(), Some("Note"), "typing goes into the note");
    // A click on an empty part of the page gives the keyboard back to the page.
    click_pt(&mut h, (250.0, 20.0));
    type_key(&mut h, egui::Key::V, "v");
    assert_eq!(h.state().quick_tool, QuickTool::Select, "V picks the Select tool");
    assert_eq!(note(&h).as_deref(), Some("Note"), "V is not typed into the note");
    type_key(&mut h, egui::Key::H, "h");
    assert_eq!(h.state().quick_tool, QuickTool::Hand, "H picks the Hand tool");
    assert_eq!(note(&h).as_deref(), Some("Note"), "nor is H");
    // Clicking back into the note types into it again, and the letters are text there.
    note_editor(&h).click();
    h.run_steps(2);
    type_key(&mut h, egui::Key::V, "v");
    assert_eq!(note(&h).as_deref(), Some("Notev"), "the note takes keys while the user types in it");
    assert_eq!(h.state().quick_tool, QuickTool::Hand, "typed letters don't pick tools");
}

#[test]
fn drawing_another_comment_takes_the_keyboard_from_a_highlight_note() {
    // #2669: a highlight's note kept the keyboard while rectangles and ovals were drawn (a drag
    // is not a click, so the editor never let go), and V and H were typed into it.
    use pdfcraft_ui_egui::comments::CommentTool;
    let mut h = harness(|app| {
        app.set_option(&egui::Context::default(), "quick", "highlight").unwrap();
        app.set_option(&egui::Context::default(), "panel", "comments").unwrap();
    });
    highlight_text(&mut h, 4, 14);
    assert!(note_editor(&h).is_focused());
    // The tools are picked without a click, as a shortcut would: the drawing alone must take
    // the keyboard back. An empty note closes.
    h.state_mut().quick_tool = QuickTool::Comment(CommentTool::Rectangle);
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    assert_eq!(comments(&h).len(), 2);
    assert_eq!(note(&h), None, "the empty note closed");
    type_key(&mut h, egui::Key::H, "h");
    assert_eq!(h.state().quick_tool, QuickTool::Hand, "H picks the Hand tool");
    // A note with text in it stays open with its text, but lets the keyboard go.
    h.state_mut().quick_tool = QuickTool::Comment(CommentTool::Highlight);
    drag_pt(&mut h, (160.0, 100.0), (260.0, 40.0));
    h.run_steps(2);
    assert_eq!(note(&h), Some(String::new()), "the new area highlight's note opens");
    assert!(note_editor(&h).is_focused());
    h.event(egui::Event::Text("Note".into()));
    h.run_steps(2);
    h.state_mut().quick_tool = QuickTool::Comment(CommentTool::Rectangle);
    drag_pt(&mut h, (40.0, 30.0), (260.0, 8.0));
    assert_eq!(comments(&h).len(), 4);
    assert_eq!(note(&h).as_deref(), Some("Note"), "a note with text stays open");
    type_key(&mut h, egui::Key::H, "h");
    assert_eq!(h.state().quick_tool, QuickTool::Hand, "H picks the Hand tool");
    assert_eq!(note(&h).as_deref(), Some("Note"), "H is not typed into the note");
}

#[test]
fn a_selection_made_first_is_marked_when_a_tool_is_picked() {
    let mut h = harness(|_| {});
    h.state_mut().views[0].select_text(0, 4, 8);
    assert!(h.execute("comment.strikeout"));
    h.run_steps(2);
    let c = comments(&h);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].subtype, "StrikeOut");
    assert!(h.state().views[0].selected_text().is_none(), "the selection is consumed");
}

#[test]
fn selected_text_shows_actions_and_underlines_without_changing_tools() {
    let mut h = harness(|_| {});
    h.state_mut().views[0].select_text(0, 4, 8);
    h.run_steps(3);
    h.get_by_label("Add comment");
    h.get_by_label("Replace text");
    h.get_by_label("Underline").click();
    h.run_steps(3);
    let c = comments(&h);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].subtype, "Underline");
    assert!(h.state().views[0].selected_text().is_none());
    assert_eq!(h.state().quick_tool, QuickTool::Select);
}

#[test]
fn selected_text_copy_action_closes_the_bar_and_keeps_the_selection() {
    let mut h = harness(|_| {});
    h.state_mut().views[0].select_text(0, 4, 8);
    h.run_steps(3);
    h.get_by_label("Copy").click();
    h.run_steps(3);
    assert!(h.query_by_label("Add comment").is_none(), "the action bar closes after Copy");
    assert!(h.state().views[0].selected_text().is_some_and(|t| !t.is_empty()), "the words stay selected");
    // A new selection brings the bar back.
    h.state_mut().views[0].select_text(0, 2, 6);
    h.run_steps(3);
    h.get_by_label("Add comment");
}

#[test]
fn dragging_over_copied_text_again_brings_the_action_bar_back() {
    let mut h = harness(|_| {});
    let (a, b) = {
        let v = &h.state().views[0];
        (v.glyph_screen_pos(0, 4).expect("text layer"), v.glyph_screen_pos(0, 8).expect("glyph"))
    };
    drag(&mut h, a, b);
    let copied = h.state().views[0].selected_text();
    assert!(copied.is_some(), "the drag selects text");
    h.get_by_label("Copy").click();
    h.run_steps(3);
    assert!(h.query_by_label("Add comment").is_none(), "the action bar closes after Copy");
    // The same drag again selects the same words, which is a new selection: the bar is back.
    drag(&mut h, a, b);
    assert_eq!(h.state().views[0].selected_text(), copied, "the drag selects the same words");
    assert!(h.query_by_label("Add comment").is_some(), "the action bar is back after a new drag");
}

/// What a row of `a_selection_action_or_tool_makes_its_comment_kind` expects.
enum Made {
    /// The Replace action opens the replacement composer.
    ReplaceComposer,
    /// The Comment action opens the note composer anchored to the words.
    NoteComposer,
    /// The engine annotation of this subtype, with one quad.
    Annotation(&'static str),
}

#[test]
fn a_selection_action_or_tool_makes_its_comment_kind() {
    use pdfcraft_ui_egui::comments::{CommentTool, ComposerKind};

    // (the way in: a selection-bar button label, or a tool command; what it makes)
    let rows: [(Result<&str, &str>, Made); 3] = [
        (Ok("Replace text"), Made::ReplaceComposer),
        (Ok("Add comment"), Made::NoteComposer),
        (Err("comment.squiggly"), Made::Annotation("Squiggly")),
    ];
    for (way, made) in rows {
        let mut h = harness(|_| {});
        h.state_mut().views[0].select_text(0, 4, 8);
        h.run_steps(3);
        match way {
            Ok(label) => {
                h.get_by_label(label).click();
                h.run_steps(2);
            }
            Err(command) => {
                assert!(h.execute(command), "{command} runs");
                h.run_steps(2);
            }
        }
        let c = comments(&h);
        match made {
            Made::ReplaceComposer => {
                h.get_by_role_and_label(Role::MultilineTextInput, "Replacement text");
                assert!(c.is_empty(), "{way:?}: nothing is posted before the replacement is typed");
            }
            Made::NoteComposer => {
                assert!(
                    matches!(h.state().views[0].session.comments.composer.as_ref().map(|c| &c.kind), Some(ComposerKind::TextNote { .. })),
                    "{way:?}: the note composer opens on the words"
                );
                assert!(h.query_by_label("Sticky note").is_none());
                assert!(c.is_empty());
            }
            Made::Annotation(subtype) => {
                assert_eq!(c.len(), 1, "{way:?}: {c:?}");
                assert_eq!((c[0].subtype.as_str(), c[0].quads.len()), (subtype, 1));
                // It sits with the other text markup in the Highlight ▸ flyout, and edits as itself.
                assert!(pdfcraft_ui_egui::comments::GROUPS[1].contains(&CommentTool::Squiggly));
                assert_eq!(pdfcraft_ui_egui::comments::tool_for(&c[0]), Some(CommentTool::Squiggly));
            }
        }
        assert!(h.state().views[0].selected_text().is_none(), "{way:?}: the selection is consumed");
    }
}

#[test]
fn selection_note_replaced_by_sticky_note_posts_at_the_new_anchor() {
    let mut h = harness(|_| {});
    h.state_mut().views[0].select_text(0, 4, 8);
    h.run_steps(3);
    h.get_by_label("Add comment").click();
    h.run_steps(2);
    h.set_option("quick", "note").unwrap();
    click_pt(&mut h, (250.0, 180.0));
    let anchor = h.state().views[0].session.comments.composer.as_ref().unwrap().at;
    field(&h, "Add a comment").click();
    h.run_steps(1);
    field(&h, "Add a comment").type_text("New sticky note");
    h.run_steps(2);
    h.get_by_label("Post").click();
    h.run_steps(3);
    let c = comments(&h);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].subtype, "Text");
    assert_eq!(c[0].contents.as_deref(), Some("New sticky note"));
    assert!(c[0].quads.is_empty());
    assert!((anchor[0] - 250.0).abs() < 1.0 && (anchor[1] - 180.0).abs() < 1.0);
    assert!((f64::from(c[0].rect[0]) - anchor[0]).abs() < 1.0 && (f64::from(c[0].rect[3]) - anchor[1]).abs() < 1.0, "{:?}", c[0].rect);
}

#[test]
fn selection_note_posts_highlight_and_hovers_author_and_comment() {
    let _gpu = std::env::var_os("PDFCRAFT_SHOTS").map(|_| crate::gpu());
    let mut h = visible_page_harness(|_| {});
    h.state_mut().views[0].select_text(0, 4, 8);
    let selected = h.state().views[0].selected_text().unwrap();
    h.run_steps(3);
    h.get_by_label("Add comment").click();
    h.run_steps(2);
    field(&h, "Add a comment").type_text("Check these words");
    h.run_steps(2);
    h.get_by_label("Post").click();
    h.run_steps(3);
    crate::support::settle(&mut h);
    let c = comments(&h);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].subtype, "Highlight");
    assert_eq!(c[0].contents.as_deref(), Some("Check these words"));
    assert!(!c[0].quads.is_empty());
    let hover = format!("{}\nCheck these words", c[0].author.as_deref().expect("comment has an author"));
    h.set_option("panel", "none").unwrap();
    h.state_mut().views[0].session.comments.clear_selection();
    h.hover_at(pos2(0.0, 0.0));
    h.run_steps(3);
    assert!(h.query_by_label(&hover).is_none());
    let r = c[0].rect;
    let p = at(&h, (r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
    h.hover_at(p);
    h.run_steps(3);
    h.get_by_label(&hover);
    assert!(h.query_by_label_contains(&selected).is_none(), "hover omits the selected text");
    if let Ok(dir) = std::env::var("PDFCRAFT_SHOTS") {
        h.render().unwrap().save(format!("{dir}/selection-comment-hover.png")).unwrap();
    }
}

#[test]
fn shapes_and_ink_are_drawn_by_dragging() {
    let mut h = visible_page_harness(|app| app.set_option(&egui::Context::default(), "quick", "square").unwrap());
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    h.set_option("quick", "ink").unwrap();
    drag_pt(&mut h, (160.0, 100.0), (260.0, 40.0));
    h.set_option("quick", "arrow").unwrap();
    drag_pt(&mut h, (20.0, 20.0), (120.0, 30.0));
    // A click with a drawing tool draws nothing.
    h.set_option("quick", "circle").unwrap();
    click_pt(&mut h, (200.0, 180.0));
    let c = comments(&h);
    let mut c = c;
    c.sort_by_key(|a| a.index);
    let kinds: Vec<&str> = c.iter().map(|a| a.subtype.as_str()).collect();
    assert_eq!(kinds, ["Square", "Ink", "Line"], "{c:?}");
    let r = c[0].rect;
    assert!((r[0] - 40.0).abs() < 3.0 && (r[1] - 40.0).abs() < 3.0 && (r[2] - 140.0).abs() < 3.0 && (r[3] - 100.0).abs() < 3.0, "{r:?}");
}

/// Press, drag and release a stroke, running only one frame after the release. On a
/// [`pen_harness`] the render jobs wait for the test, so the raster the commit asks for can't
/// arrive before the test looks, however fast it renders; a test lets it through with
/// `support::settle_held`.
fn stroke_without_settling(h: &mut Harness<'static, PdfCraftApp>, a: (f32, f32), b: (f32, f32)) {
    let (from, to) = (at(h, a.0, a.1), at(h, b.0, b.1));
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(from + (to - from) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    h.drop_at(to);
    h.run_steps(1);
}

/// The pen's colour and width (points) in the pen tests: distinctive, so that only the live
/// stroke's painted path matches (the page raster is a texture, never a path).
const PEN_COLOUR: [f64; 3] = [0.2, 0.4, 0.6];
const PEN_WIDTH: f64 = 7.0;

/// A harness with the Ink tool armed in the pen colour and width above, whose jobs (the page
/// renders among them) wait until the test runs them; the pages are rendered once before it returns.
fn pen_harness() -> (Harness<'static, PdfCraftApp>, std::sync::Arc<craft::host::ManualJobs>) {
    let (app, jobs) = crate::control::app_with_manual_jobs();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = app;
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "text.pdf", None, TEXT_FIXTURE.to_vec()).expect("opens");
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "author", "Tester").unwrap();
            app.comment_prefs.set_color(pdfcraft_ui_egui::comments::CommentTool::Ink, PEN_COLOUR);
            app.comment_prefs.set_width(pdfcraft_ui_egui::comments::CommentTool::Ink, PEN_WIDTH);
            app.set_option(&egui::Context::default(), "quick", "ink").unwrap();
            app
        }
    });
    h.run_steps(4);
    support::settle_held(&mut h, &jobs);
    (h, jobs)
}

/// How many path shapes of the last frame are painted in the pen's colour and width.
fn pen_paths(h: &Harness<'static, PdfCraftApp>) -> usize {
    fn walk(shape: &egui::Shape, colour: egui::Color32, width: f32, out: &mut usize) {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, colour, width, out)),
            egui::Shape::Path(p) if p.stroke.color == egui::epaint::ColorMode::Solid(colour) && (p.stroke.width - width).abs() < 0.6 => *out += 1,
            _ => {}
        }
    }
    let page = h.state().views[0].page_screen_rect(0).expect("page 1 on screen");
    let width = PEN_WIDTH as f32 * page.width() / 300.0;
    let colour = pdfcraft_ui_egui::comments::color32(PEN_COLOUR);
    let mut out = 0;
    for clipped in &h.output().shapes {
        walk(&clipped.shape, colour, width, &mut out);
    }
    out
}

/// #1858: on release the live stroke is replaced by the committed annotation, but the page
/// raster that draws it arrives later. The stroke stays painted from its points meanwhile.
#[test]
fn a_released_pen_stroke_stays_drawn_until_the_page_raster_has_it() {
    let (mut h, jobs) = pen_harness();
    assert_eq!(pen_paths(&h), 0, "nothing in the pen's colour before the stroke");
    stroke_without_settling(&mut h, (40.0, 100.0), (140.0, 60.0));
    assert_eq!(comments(&h).len(), 1, "the stroke is committed");
    assert!(pen_paths(&h) > 0, "still drawn from its points");
    h.run_steps(5);
    assert!(pen_paths(&h) > 0, "drawn from its points for as long as its raster takes");
    support::settle_held(&mut h, &jobs);
    assert!(support::step_until(&mut h, |h| pen_paths(h) == 0), "the raster draws it now and the painted stroke goes");
}

#[test]
fn undoing_a_pen_stroke_before_its_raster_leaves_no_ghost() {
    let (mut h, _jobs) = pen_harness();
    stroke_without_settling(&mut h, (40.0, 100.0), (140.0, 60.0));
    assert!(pen_paths(&h) > 0, "the released stroke is painted");
    undo(&mut h);
    h.run_steps(2);
    assert_eq!(comments(&h).len(), 0, "the stroke is undone");
    assert_eq!(pen_paths(&h), 0, "nothing of it is left painted while its raster is held back");
}

#[test]
fn pen_strokes_stay_unselected_so_writing_continues() {
    let mut h = visible_page_harness(|app| app.set_option(&egui::Context::default(), "quick", "ink").unwrap());
    // No Comments panel: the author's name should only show in a hover popup.
    h.set_option("panel", "none").unwrap();
    h.run_steps(2);
    drag_pt(&mut h, (40.0, 100.0), (140.0, 60.0));
    assert!(h.state().views[0].session.comments.selection().is_empty(), "the first stroke isn't selected");
    // The pointer still rests on the stroke: no author popup covers it.
    h.hover_at(at(&h, 140.0, 60.0));
    h.run_steps(3);
    assert!(h.query_by_label_contains("Tester").is_none(), "no author popup while drawing");
    drag_pt(&mut h, (60.0, 100.0), (160.0, 60.0));
    let c = comments(&h);
    let kinds: Vec<&str> = c.iter().map(|a| a.subtype.as_str()).collect();
    assert_eq!(kinds, ["Ink", "Ink"], "{c:?}");
    assert!(h.state().views[0].session.comments.selection().is_empty(), "nothing is selected after drawing");
    assert_eq!(h.state().quick_tool, QuickTool::Comment(pdfcraft_ui_egui::comments::CommentTool::Ink), "the pen stays on");
    // Other drawn shapes are still selected when made.
    h.set_option("quick", "square").unwrap();
    drag_pt(&mut h, (180.0, 100.0), (260.0, 40.0));
    assert_eq!(h.state().views[0].session.comments.selection(), vec![(0, 2)]);
}

#[test]
fn sticky_note_composer_posts_and_returns_to_select() {
    let mut h = harness(|app| app.set_option(&egui::Context::default(), "quick", "note").unwrap());
    click_pt(&mut h, (250.0, 180.0));
    assert!(h.state().views[0].session.comments.composer.is_some(), "the composer opened");
    field(&h, "Add a comment").type_text("Please check");
    h.run_steps(2);
    h.get_by_label("Post").click();
    h.run_steps(3);
    let c = comments(&h);
    assert_eq!(c.len(), 1);
    assert_eq!((c[0].subtype.as_str(), c[0].contents.as_deref()), ("Text", Some("Please check")));
    assert!(h.state().views[0].session.comments.composer.is_none());
    assert_eq!(h.state().quick_tool, QuickTool::Select, "one-shot tools return to Select");
    // Cancelling another one creates nothing.
    h.set_option("quick", "freetext").unwrap();
    click_pt(&mut h, (50.0, 60.0));
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert_eq!(comments(&h).len(), 1);
}

/// #2583: a sticky note is saved NoRotate, so on a turned page its icon is drawn upright from
/// where it was placed (right and down), not where its `/Rect` lies as displayed. The Select tool
/// picks it where it is drawn.
#[test]
fn a_sticky_note_on_a_turned_page_is_picked_where_it_is_drawn() {
    let mut h = harness(|app| {
        app.apply_edit(&egui::Context::default(), pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
        app.set_option(&egui::Context::default(), "quick", "note").unwrap();
    });
    crate::support::settle(&mut h);
    let page = h.state().views[0].page_screen_rect(0).expect("page 1 on screen");
    // Screen pixels per point of the displayed page (300 × 200 pt turned to 200 × 300).
    let k = page.width() / 200.0;
    let placed = page.left_top() + egui::vec2(100.0 * k, 60.0 * k);
    click(&mut h, placed);
    field(&h, "Add a comment").type_text("Turned");
    h.run_steps(2);
    h.get_by_label("Post").click();
    h.run_steps(3);
    crate::support::settle(&mut h);
    let c = comments(&h);
    assert_eq!(c.len(), 1);
    assert_eq!((c[0].subtype.as_str(), c[0].no_rotate), ("Text", true));
    assert_eq!(h.state().quick_tool, QuickTool::Select);
    // Where the note's /Rect lies as displayed, left of where it was placed: nothing.
    click(&mut h, placed + egui::vec2(-10.0 * k, 10.0 * k));
    assert!(h.state().views[0].session.comments.selection().is_empty());
    // On the icon, drawn right and down from where it was placed.
    click(&mut h, placed + egui::vec2(10.0 * k, 10.0 * k));
    assert_eq!(h.state().views[0].session.comments.selection(), vec![(0, 0)]);
}

/// The composer leaves with its comment when the page scrolls it out of the document, and comes
/// back with the draft (#4021). It used to be pushed back into the window, over the mode bar and
/// the toolbar, while its comment was out of sight.
#[test]
fn the_composer_hides_while_its_comment_is_scrolled_away() {
    let mut h = harness(|app| app.set_option(&egui::Context::default(), "quick", "note").unwrap());
    click_pt(&mut h, (250.0, 180.0));
    field(&h, "Add a comment").type_text("Draft");
    h.run_steps(2);
    let editors = |h: &Harness<'static, PdfCraftApp>| -> Vec<egui::Rect> {
        h.query_all_by(|n| n.role() == egui::accesskit::Role::MultilineTextInput && !n.is_hidden()).map(|n| n.rect()).collect()
    };
    let viewport = h.state().views[0].viewport_rect();
    assert!(editors(&h).iter().any(|r| viewport.contains(r.center())), "the composer opens in the document");
    // The page scrolled up: it still shows, the comment near its top doesn't.
    h.state_mut().views[0].view.go_to_fraction(0, 0.4);
    h.run_steps(4);
    assert!(h.state().views[0].page_screen_rect(0).is_some_and(|r| r.bottom() > viewport.top()), "page 1 still shows");
    assert!(at(&h, 250.0, 180.0).y < viewport.top(), "the comment is scrolled away");
    let shown = editors(&h);
    assert!(shown.is_empty(), "the composer stays with its comment, not over the toolbar: {shown:?} (document {viewport:?})");
    // Back again, the draft is still there.
    h.state_mut().views[0].view.go_to_fraction(0, 0.0);
    h.run_steps(4);
    assert!(editors(&h).iter().any(|r| viewport.contains(r.center())), "the composer is back");
    assert_eq!(h.state().views[0].session.comments.composer.as_ref().map(|c| c.text.as_str()), Some("Draft"));
}

/// Editing a large comment whose top-right corner is scrolled away still shows the composer
/// while the rest of the comment is on screen, so it can be posted or cancelled (#4095).
#[test]
fn the_edit_composer_shows_while_part_of_its_comment_is_on_screen() {
    let mut h = harness(|app| app.set_option(&egui::Context::default(), "quick", "square").unwrap());
    drag_pt(&mut h, (20.0, 190.0), (280.0, 10.0));
    // What "Edit text…" opens: the composer anchored at the comment's top-right corner.
    h.state_mut().views[0].session.comments.composer = Some(pdfcraft_ui_egui::comments::Composer {
        page: 0,
        at: [280.0, 190.0],
        kind: pdfcraft_ui_egui::comments::ComposerKind::Edit(0),
        text: String::new(),
        focus: true,
    });
    h.run_steps(2);
    let viewport = h.state().views[0].viewport_rect();
    h.state_mut().views[0].view.go_to_fraction(0, 0.4);
    h.run_steps(4);
    assert!(at(&h, 280.0, 190.0).y < viewport.top(), "the comment's top-right corner is scrolled away");
    assert!(at(&h, 280.0, 10.0).y > viewport.top(), "part of the comment is still on screen");
    let editors: Vec<_> = h.query_all_by(|n| n.role() == egui::accesskit::Role::MultilineTextInput).map(|n| n.rect()).collect();
    assert!(editors.iter().any(|r| viewport.contains(r.center())), "the editor is drawn: {editors:?} (document {viewport:?})");
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert!(h.state().views[0].session.comments.composer.is_none(), "Cancel closes it");
}

/// The rectangle of the comment whose text starts with `text`.
fn rect_of(h: &Harness<'static, PdfCraftApp>, text: &str) -> [f32; 4] {
    let c = comments(h);
    c.iter().find(|a| a.contents.as_deref().is_some_and(|t| t.starts_with(text))).unwrap_or_else(|| panic!("no comment {text:?}: {c:?}")).rect
}

/// Post `text` in the open text box composer.
fn post_text(h: &mut Harness<'static, PdfCraftApp>, text: &str) {
    field(h, "Type text").type_text(text);
    h.run_steps(2);
    h.get_by_label("Post").click();
    h.run_steps(3);
}

fn near(r: [f32; 4], want: [f32; 4]) -> bool {
    r.iter().zip(want).all(|(a, b)| (a - b).abs() < 1.5)
}

/// #2669: a drag with Add a text box draws the box at that size, and the text wraps inside it;
/// a click (or a drag of a few pixels) still makes a box sized to its text.
#[test]
fn a_text_box_is_sized_by_dragging() {
    let mut h = harness(|app| app.set_option(&egui::Context::default(), "quick", "freetext").unwrap());
    let (from, to) = (at(&h, 40.0, 150.0), at(&h, 200.0, 100.0));
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    h.hover_at(to);
    h.run_steps(2);
    h.drop_at(to);
    h.run_steps(3);
    post_text(&mut h, "Drawn");
    let r = rect_of(&h, "Drawn");
    assert!(near(r, [40.0, 100.0, 200.0, 150.0]), "{r:?}");

    // Too much text for the drawn box: it keeps its width and grows down, moving up so that it
    // stays on the page (whose bottom is at 0).
    h.set_option("quick", "freetext").unwrap();
    drag_pt(&mut h, (20.0, 80.0), (120.0, 70.0));
    post_text(&mut h, &"word ".repeat(20));
    let r = rect_of(&h, "word word");
    assert!((r[0] - 20.0).abs() < 1.5 && (r[2] - 120.0).abs() < 1.5 && r[1].abs() < 0.5 && r[3] > 90.0, "{r:?}");

    // More text than the page can hold: the box keeps the drawn size and the text is clipped.
    h.set_option("quick", "freetext").unwrap();
    drag_pt(&mut h, (150.0, 190.0), (250.0, 180.0));
    post_text(&mut h, &"longerwords ".repeat(16));
    let r = rect_of(&h, "longerwords");
    assert!(near(r, [150.0, 180.0, 250.0, 190.0]), "{r:?}");

    // Drags of two and four pixels are clicks: the box fits its text.
    let fitted = pdfcraft_ui_egui::comments::text_box_rect([0.0, 0.0], "Hi", 12.0, 0);
    for (text, x, d) in [("Hi", 200.0, 2.0), ("Hi again", 270.0, 4.0)] {
        h.set_option("quick", "freetext").unwrap();
        let p = at(&h, x, 40.0);
        drag(&mut h, p, p + egui::vec2(d, d));
        post_text(&mut h, text);
        let r = rect_of(&h, text).map(f64::from);
        let fitted = if text == "Hi" { fitted } else { pdfcraft_ui_egui::comments::text_box_rect([0.0, 0.0], text, 12.0, 0) };
        assert!((r[2] - r[0] - (fitted[2] - fitted[0])).abs() < 1.0 && (r[3] - r[1] - (fitted[3] - fitted[1])).abs() < 1.0, "{text}: {r:?}");
    }
}

/// #2669: with the view turned (View > Rotate View) the box lands where it was drawn, not
/// transposed or scaled by the turned page's other side.
#[test]
fn a_dragged_text_box_follows_a_rotated_view() {
    for rotation in [90u16, 270] {
        let mut h = harness(|app| app.set_option(&egui::Context::default(), "quick", "freetext").unwrap());
        while h.state().views[0].view.rotation != rotation {
            let view = &mut h.state_mut().views[0].view;
            view.rotation = (view.rotation + 90) % 360;
        }
        h.set_option("zoom", "150").unwrap();
        h.run_steps(3);
        crate::support::settle(&mut h);
        let screen = |h: &Harness<'static, PdfCraftApp>, x: f32, y: f32| {
            let r = h.state().views[0].page_screen_rect(0).expect("page 1 on screen");
            let (u, v) = (x / 300.0, (200.0 - y) / 200.0);
            let (a, b) = if rotation == 90 { (1.0 - v, u) } else { (v, 1.0 - u) };
            pos2(r.left() + a * r.width(), r.top() + b * r.height())
        };
        let (from, to) = (screen(&h, 40.0, 150.0), screen(&h, 200.0, 100.0));
        drag(&mut h, from, to);
        post_text(&mut h, "Turned");
        let r = rect_of(&h, "Turned");
        assert!(near(r, [40.0, 100.0, 200.0, 150.0]), "view turned {rotation}: {r:?}");
    }
}

/// #2669: the text of a drawn box wraps inside its border, as its appearance draws it, so a thick
/// border does not cut off the last line.
#[test]
fn a_dragged_text_box_wraps_inside_its_border() {
    use pdfcraft_ui_egui::comments::CommentTool;
    let mut h = harness(|app| {
        app.set_option(&egui::Context::default(), "quick", "freetext").unwrap();
        app.comment_prefs.set_width(CommentTool::TextBox, 3.0);
    });
    // Wide enough for "word word" inside a 2 pt padding, not inside 2 + 3 pt.
    let w = pdfcraft_engine::annot_text::text_width("word word", 12.0) as f32 + 6.0;
    drag_pt(&mut h, (20.0, 190.0), (20.0 + w, 180.0));
    post_text(&mut h, "word word");
    let r = rect_of(&h, "word word");
    assert!((r[3] - r[1] - (2.0 * 14.4 + 2.0 * 5.0 + 2.0)).abs() < 1.0, "two lines inside the border: {r:?}");
}

#[test]
fn comments_are_selected_moved_and_deleted_with_the_select_tool() {
    let mut h = visible_page_harness(|app| app.set_option(&egui::Context::default(), "quick", "square").unwrap());
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    h.set_option("quick", "select").unwrap();
    // Click away, then on the rectangle's border area.
    click_pt(&mut h, (250.0, 20.0));
    assert!(h.state().views[0].session.comments.selection().is_empty());
    click_pt(&mut h, (90.0, 70.0));
    assert_eq!(h.state().views[0].session.comments.selection(), vec![(0, 0)]);
    drag_pt(&mut h, (90.0, 70.0), (140.0, 70.0));
    let r = comments(&h)[0].rect;
    assert!((r[0] - 90.0).abs() < 3.0 && (r[2] - 190.0).abs() < 3.0, "moved right by 50 pt: {r:?}");
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Move comment"));
    h.key_press(egui::Key::Delete);
    h.run_steps(3);
    assert!(comments(&h).is_empty());
    undo(&mut h);
    h.run_steps(2);
    assert_eq!(comments(&h).len(), 1);
}

/// A line's ends have handles: dragging one with the Select tool changes the line's length and
/// angle (Shift holds it level or plumb with the other end), as one undo step. The line tool keeps
/// drawing from an end: a new line starts there rather than dragging the one just drawn.
#[test]
fn line_ends_are_dragged_to_change_length_and_angle() {
    let mut h = harness(|app| app.set_option(&egui::Context::default(), "quick", "line").unwrap());
    drag_pt(&mut h, (40.0, 100.0), (140.0, 100.0));
    let line = comments(&h).into_iter().find(|a| a.subtype == "Line").expect("a line");
    assert_eq!(line.points.len(), 2);
    let ends = |h: &Harness<'static, PdfCraftApp>| comments(h).into_iter().find(|a| a.index == line.index).unwrap().points;
    // With the line tool, a drag from the end just drawn draws another line.
    let first = ends(&h);
    drag_pt(&mut h, (first[1][0], first[1][1]), (140.0, 40.0));
    assert_eq!(comments(&h).iter().filter(|a| a.subtype == "Line").count(), 2, "a second line");
    assert_eq!(ends(&h), first, "the first one stays");
    h.set_option("quick", "select").unwrap();
    click_pt(&mut h, (90.0, 100.0));
    assert_eq!(h.state().views[0].session.comments.primary(), Some((0, line.index)));
    // Drag the right end up and further right: longer, and turned.
    let [_, end] = [line.points[0], line.points[1]];
    drag_pt(&mut h, (end[0], end[1]), (200.0, 160.0));
    let p = ends(&h);
    assert!((p[0][0] - 40.0).abs() < 1.0 && (p[0][1] - 100.0).abs() < 1.0, "the other end stays: {p:?}");
    assert!((p[1][0] - 200.0).abs() < 1.0 && (p[1][1] - 160.0).abs() < 1.0, "the dragged end follows: {p:?}");
    let id = h.state().views[0].id;
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Reshape comment"));
    // Shift: held level or plumb with the other end, here level (110 pt across, 90 up).
    let a = at(&h, 200.0, 160.0);
    let b = at(&h, 150.0, 190.0);
    h.hover_at(a);
    h.run_steps(1);
    h.event(egui::Event::ModifiersChanged(egui::Modifiers::SHIFT));
    h.drag_at(a);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(a + (b - a) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    h.drop_at(b);
    h.run_steps(3);
    h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    h.run_steps(1);
    let p = ends(&h);
    assert!((p[1][1] - p[0][1]).abs() < 0.01 && (p[1][0] - 150.0).abs() < 1.0, "level: {p:?}");
    // Undo puts the line back as it was before the drag.
    undo(&mut h);
    h.run_steps(2);
    let p = ends(&h);
    assert!((p[1][0] - 200.0).abs() < 1.0 && (p[1][1] - 160.0).abs() < 1.0, "{p:?}");
}

#[test]
fn arrow_keys_nudge_all_selected_comments_as_one_undo_step() {
    use egui::Key;
    let mut h = harness(|app| app.set_option(&egui::Context::default(), "quick", "square").unwrap());
    drag_pt(&mut h, (30.0, 110.0), (80.0, 70.0));
    drag_pt(&mut h, (130.0, 110.0), (180.0, 70.0));
    h.set_option("quick", "select").unwrap();
    let before: Vec<_> = comments(&h).iter().map(|a| a.rect).collect();
    assert_eq!(before.len(), 2);
    h.state_mut().views[0].session.comments.select_only((0, 0));
    h.state_mut().views[0].session.comments.toggle((0, 1));
    h.key_press(Key::ArrowRight);
    h.run_steps(3);
    let moved: Vec<_> = comments(&h).iter().map(|a| a.rect).collect();
    for (old, new) in before.iter().zip(&moved) {
        assert!(new[0] > old[0], "each selected comment moves: {before:?} -> {moved:?}");
    }
    undo(&mut h);
    h.run_steps(2);
    let undone: Vec<_> = comments(&h).iter().map(|a| a.rect).collect();
    assert_eq!(undone, before, "one undo restores the whole selection");
}

#[test]
fn arrow_nudge_skips_locked_comments_and_duplicate_selection_entries() {
    let mut h = visible_page_harness(|app| app.set_option(&egui::Context::default(), "quick", "square").unwrap());
    for (a, b) in [((20.0, 20.0), (60.0, 60.0)), ((100.0, 20.0), (140.0, 60.0)), ((180.0, 20.0), (220.0, 60.0))] {
        drag_pt(&mut h, a, b);
    }
    h.set_option("quick", "select").unwrap();
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().apply_edit(&ctx_h, pdfcraft_engine::Edit::LockAnnotation { page: 0, index: 1, locked: true }));
    h.run_steps(2);
    let before = comments(&h);
    // A repeated key is no selection entry: `toggle` takes a selected comment out again.
    h.state_mut().views[0].session.comments.select_only((0, 0));
    for key in [(0, 1), (0, 2)] {
        h.state_mut().views[0].session.comments.toggle(key);
    }
    assert_eq!(h.state().views[0].session.comments.selection().len(), 3);
    h.key_press(egui::Key::ArrowRight);
    h.run_steps(3);
    let after = comments(&h);
    let left = |list: &[Annotation], i: usize| list.iter().find(|a| a.index == i).expect("comment still exists").rect[0];
    let primary_delta = left(&after, 0) - left(&before, 0);
    let secondary_delta = left(&after, 2) - left(&before, 2);
    assert!(primary_delta > 0.0, "primary comment moves");
    assert!((secondary_delta - primary_delta).abs() < 0.01, "every selected comment moves once");
    assert_eq!(left(&after, 1), left(&before, 1), "locked comment stays in place");
    undo(&mut h);
    h.run_steps(2);
    let undone = comments(&h);
    for i in 0..3 {
        assert_eq!(left(&undone, i), left(&before, i), "one undo restores comment {i}");
    }
}

/// Over the first corner, where a click closes the polygon, the rubber band runs to that corner
/// with Shift held too: the closing side the click will make, not a level or plumb one.
#[test]
fn the_rubber_band_shows_the_closing_side_with_shift_held() {
    let _gpu = crate::gpu();
    let mut h = harness(|app| app.set_option(&egui::Context::default(), "quick", "polygon").unwrap());
    for p in [(40.0, 40.0), (140.0, 40.0), (140.0, 140.0)] {
        click_pt(&mut h, p);
    }
    h.event(egui::Event::ModifiersChanged(egui::Modifiers::SHIFT));
    h.hover_at(at(&h, 40.0, 40.0) + egui::vec2(2.0, 2.0));
    h.run_steps(3);
    let pixels = h.render().unwrap();
    // Halfway along the diagonal closing side, (90, 90).
    let m = at(&h, 90.0, 90.0);
    let drawn = (-2..=2).any(|d: i32| {
        let v = pixels.get_pixel((m.x as i32 + d) as u32, (m.y as i32 + d) as u32).0;
        v[0] > 150 && v[1] < 100 && v[2] < 100
    });
    assert!(drawn, "the closing side is drawn");
    h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    h.run_steps(1);
}

/// Shift holds a new line or arrow level or plumb with where it starts, and each new side of
/// connected lines or a polygon with the corner before (a CAD drawing's ortho mode). Without
/// Shift they go where the pointer is.
#[test]
fn shift_holds_new_lines_and_sides_level_or_plumb() {
    let mut h = visible_page_harness(|app| app.set_option(&egui::Context::default(), "quick", "line").unwrap());
    let shift = |h: &mut Harness<'static, PdfCraftApp>, on: bool| {
        h.event(egui::Event::ModifiersChanged(if on { egui::Modifiers::SHIFT } else { egui::Modifiers::NONE }));
        h.run_steps(1);
    };
    let points = |h: &Harness<'static, PdfCraftApp>, subtype: &str| {
        // In the order drawn: the list isn't sorted by index.
        let mut drawn: Vec<_> = comments(h).into_iter().filter(|a| a.subtype == subtype).collect();
        drawn.sort_by_key(|a| a.index);
        drawn.into_iter().map(|a| a.points).collect::<Vec<_>>()
    };
    let close = |p: [f32; 2], q: [f32; 2]| (p[0] - q[0]).abs() < 1.0 && (p[1] - q[1]).abs() < 1.0;
    shift(&mut h, true);
    drag_pt(&mut h, (40.0, 100.0), (140.0, 120.0));
    drag_pt(&mut h, (200.0, 40.0), (215.0, 150.0));
    shift(&mut h, false);
    drag_pt(&mut h, (40.0, 160.0), (140.0, 190.0));
    let lines = points(&h, "Line");
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert!(close(lines[0][0], [40.0, 100.0]) && close(lines[0][1], [140.0, 100.0]), "mostly across: level: {lines:?}");
    assert!(close(lines[1][0], [200.0, 40.0]) && close(lines[1][1], [200.0, 150.0]), "mostly up: plumb: {lines:?}");
    assert!(close(lines[2][1], [140.0, 190.0]), "no Shift, no hold: {lines:?}");

    h.set_option("quick", "arrow").unwrap();
    shift(&mut h, true);
    drag_pt(&mut h, (260.0, 180.0), (150.0, 170.0));
    shift(&mut h, false);
    let arrow = points(&h, "Line").pop().expect("an arrow");
    assert!(close(arrow[1], [150.0, 180.0]), "an arrow too, leftwards: {arrow:?}");

    // Connected lines: each click with Shift is held to the corner before.
    h.set_option("quick", "polyline").unwrap();
    click_pt(&mut h, (20.0, 20.0));
    shift(&mut h, true);
    // Pointer events carry the modifier held at each press, as real input does.
    click_pt_with(&mut h, (90.0, 30.0), egui::Modifiers::SHIFT);
    click_pt_with(&mut h, (95.0, 80.0), egui::Modifiers::SHIFT);
    shift(&mut h, false);
    click_pt(&mut h, (130.0, 60.0));
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    let poly = points(&h, "PolyLine").pop().expect("connected lines");
    assert_eq!(poly.len(), 4, "{poly:?}");
    assert!(close(poly[1], [90.0, 20.0]) && close(poly[2], [90.0, 80.0]) && close(poly[3], [130.0, 60.0]), "{poly:?}");
}

#[test]
fn a_shape_just_drawn_resizes_from_its_handles_without_the_select_tool() {
    // #2338: the new shape showed its handles, but only the Select tool answered them.
    let square = QuickTool::Comment(pdfcraft_ui_egui::comments::CommentTool::Rectangle);
    let mut h = visible_page_harness(|app| app.set_option(&egui::Context::default(), "quick", "square").unwrap());
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    assert_eq!(h.state().views[0].session.comments.selection(), vec![(0, 0)], "the new rectangle is selected");
    let before = comments(&h)[0].rect;
    // Drag its bottom-right handle 40 pt right and 20 pt down.
    drag_pt(&mut h, (before[2], before[1]), (before[2] + 40.0, before[1] - 20.0));
    let c = comments(&h);
    assert_eq!(c.len(), 1, "the handle resized the rectangle instead of drawing another: {c:?}");
    let r = c[0].rect;
    assert!((r[2] - (before[2] + 40.0)).abs() < 3.0 && (r[1] - (before[1] - 20.0)).abs() < 3.0, "resized: {before:?} → {r:?}");
    assert!((r[0] - before[0]).abs() < 1.0 && (r[3] - before[3]).abs() < 1.0, "the opposite corner stays: {before:?} → {r:?}");
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Resize comment"));
    assert_eq!(h.state().quick_tool, square, "the rectangle tool stays on");
    // Away from the handles the tool still draws, over the shape too.
    drag_pt(&mut h, (60.0, 90.0), (120.0, 60.0));
    assert_eq!(comments(&h).len(), 2, "a drag inside the rectangle draws a second one");
}

/// Row of `panel_and_card_actions_become_engine_edits`: the panel's add box and a card's reply box
/// give the engine an `Add` and a `Reply`.
fn panel_posts_comment_and_reply_row() {
    let mut h = harness(|app| app.set_option(&egui::Context::default(), "panel", "comments").unwrap());
    field(&h, "Add a comment").click();
    h.run_steps(2);
    field(&h, "Add a comment").type_text("General remark");
    h.run_steps(1);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    let c = comments(&h);
    assert_eq!((c.len(), c[0].subtype.as_str(), c[0].contents.as_deref()), (1, "Text", Some("General remark")));
    // The new comment is selected: reply to it.
    field(&h, "Add a reply").click();
    h.run_steps(2);
    field(&h, "Add a reply").type_text("Agreed");
    h.run_steps(1);
    h.get_by_label("Post").click();
    h.run_steps(3);
    let c = comments(&h);
    assert_eq!(c.len(), 2);
    let reply = c.iter().find(|a| a.in_reply_to.is_some()).expect("a reply");
    assert_eq!((reply.contents.as_deref(), reply.author.as_deref()), (Some("Agreed"), Some("Tester")));
    h.get_by_label_contains("Agreed");
}

// PLAN: pdf-comments-w11 keeps these row keys private to `dialogs/comment_props.rs`; an integration test cannot import a
// `pub(crate)` constant, so w11 must make them `pub` for this file to share them. Assumed
// `PdfCraftApp::comment_props: Option<CommentPropsDialog { id, .. }>` and
// `PdfCraftApp::dialogs.get_mut_by_id::<HostedParamDialog>(id)` for the hosted Comment Properties
// dialog (pdf-comments-w11); the row keys are the dialog's own (`comment.props.colour`, ...).
const PROP_COLOUR: &str = "comment.props.colour";
const PROP_BORDER: &str = "comment.props.border";
const PROP_FILL: &str = "comment.props.fill";

/// Write the row `key` of the open Comment Properties dialog; the dialog shows it in its next pass.
fn set_prop(h: &mut Harness<'static, PdfCraftApp>, key: &str, value: FieldValue) {
    let app = h.state_mut();
    let id = app.comment_props.as_ref().expect("Comment Properties is open").id;
    app.dialogs.get_mut_by_id::<HostedParamDialog>(id).expect("the hosted dialog is open").set_field(key, value);
    h.run_steps(2);
}

/// Take the value of the row `key` of the open Comment Properties dialog away ("No fill").
fn clear_prop(h: &mut Harness<'static, PdfCraftApp>, key: &str) {
    let app = h.state_mut();
    let id = app.comment_props.as_ref().expect("Comment Properties is open").id;
    app.dialogs.get_mut_by_id::<HostedParamDialog>(id).expect("the hosted dialog is open").clear_field(key);
    h.run_steps(2);
}

/// Accept the open Comment Properties dialog with its OK button.
fn accept_props(h: &mut Harness<'static, PdfCraftApp>) {
    h.get_by_label("OK").click();
    h.run_steps(3);
}

#[test]
fn line_ending_properties_change_and_undo() {
    let mut h = harness(|app| app.set_option(&egui::Context::default(), "quick", "line").unwrap());
    drag_pt(&mut h, (40.0, 100.0), (160.0, 40.0));
    open_comment_props(&mut h, 0, 0);
    h.run_steps(2);
    h.get_by_label("Line ending");
    set_prop(&mut h, "ending.1", FieldValue::Text(pdfcraft_engine::LineEnding::Diamond.name().into()));
    accept_props(&mut h);
    let endings = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        assert_eq!(doc.can_undo(), Some("Change comment properties"));
        doc.comment_props(0, 0).unwrap().endings
    };
    assert_eq!(endings.as_deref(), Some(&[pdfcraft_engine::LineEnding::None, pdfcraft_engine::LineEnding::Diamond][..]));
    h.execute("edit.undo");
    h.run_steps(2);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(
        doc.comment_props(0, 0).unwrap().endings.as_deref(),
        Some(&[pdfcraft_engine::LineEnding::None, pdfcraft_engine::LineEnding::None][..])
    );
}

#[test]
fn comment_properties_change_appearance_and_author() {
    let mut h = harness(|app| app.set_option(&egui::Context::default(), "quick", "square").unwrap());
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    open_comment_props(&mut h, 0, 0);
    h.run_steps(2);
    h.get_by_role_and_label(Role::Dialog, "Rectangle Properties");
    {
        let s = h.state();
        assert_eq!(s.session.get(s.views[0].id).unwrap().comment_props(0, 0).unwrap().width, Some(2.0));
    }
    set_prop(&mut h, PROP_COLOUR, FieldValue::Color([0.0, 0.0, 1.0]));
    set_prop(&mut h, "width", FieldValue::Number(5.0));
    h.get_by_label("General").click();
    h.run_steps(2);
    h.get_by_label("Author");
    set_prop(&mut h, "author", FieldValue::Text("Grace".into()));
    accept_props(&mut h);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    let p = doc.comment_props(0, 0).unwrap();
    assert_eq!((p.color, p.width, p.author.as_str()), (Some([0.0, 0.0, 1.0]), Some(5.0), "Grace"));
    assert_eq!(doc.can_undo(), Some("Change comment properties"), "one undo step");
}

/// #686: rectangles, ovals, polygons and clouds can be filled, from the quick bar for new ones
/// and from Properties for existing ones; "No fill" takes it away again.
#[test]
fn shapes_take_a_fill_colour_from_the_quick_bar_and_properties() {
    use pdfcraft_ui_egui::comments::CommentTool;
    let mut h = harness(|app| app.set_option(&egui::Context::default(), "quick", "square").unwrap());
    h.run_steps(2);
    // No fill by default; the quick bar says so.
    h.get_by_label("Fill colour: none");
    h.state_mut().comment_prefs.set_fill(CommentTool::Rectangle, Some([1.0, 0.5, 0.0]));
    // Tools without an interior keep no fill.
    h.state_mut().comment_prefs.set_fill(CommentTool::Line, Some([1.0, 0.5, 0.0]));
    assert_eq!(h.state().comment_prefs.style(CommentTool::Line).fill, None);
    h.run_steps(2);
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    let fill_of = |h: &Harness<'static, PdfCraftApp>, index: usize| {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let p = doc.comment_props(0, index).unwrap();
        assert!(p.fillable);
        p.fill
    };
    assert_eq!(fill_of(&h, 0), Some([1.0, 0.5, 0.0]), "the new rectangle is filled");
    // Quick bar ▸ Fill colour ▸ No fill: the next rectangle has none.
    h.set_option("quick", "square").unwrap();
    h.run_steps(2);
    h.get_by_label("Fill colour").click();
    h.run_steps(2);
    h.get_by_label("No fill").click();
    h.run_steps(2);
    assert_eq!(h.state().comment_prefs.style(CommentTool::Rectangle).fill, None);
    drag_pt(&mut h, (160.0, 100.0), (260.0, 40.0));
    assert_eq!(fill_of(&h, 1), None);

    // Properties ▸ Fill colour changes an existing one, as one undo step.
    open_comment_props(&mut h, 0, 0);
    h.run_steps(2);
    h.get_by_role_and_label(Role::Dialog, "Rectangle Properties");
    h.get_by_label("Fill colour");
    clear_prop(&mut h, PROP_FILL);
    accept_props(&mut h);
    assert_eq!(fill_of(&h, 0), None);
    let undo = h.state().session.get(h.state().views[0].id).unwrap().can_undo().map(str::to_owned);
    assert_eq!(undo.as_deref(), Some("Change comment properties"));
    open_comment_props(&mut h, 0, 1);
    h.run_steps(2);
    set_prop(&mut h, PROP_FILL, FieldValue::Color([0.0, 0.0, 1.0]));
    set_prop(&mut h, "make_default", FieldValue::Bool(true));
    accept_props(&mut h);
    assert_eq!(fill_of(&h, 1), Some([0.0, 0.0, 1.0]));
    // Make Properties Default carries the fill into the tool.
    assert_eq!(h.state().comment_prefs.style(CommentTool::Rectangle).fill, Some([0.0, 0.0, 1.0]));
}

/// Comment Properties ▸ Border shape puts an oval or a circle around a text box's text; a
/// callout has none to choose. Make Properties Default carries it into the Text box tool.
#[test]
fn text_box_properties_choose_the_border_shape() {
    use pdfcraft_engine::BorderShape;
    use pdfcraft_ui_egui::comments::CommentTool;
    let mut h = harness(|_| {});
    let shape = pdfcraft_engine::Shape::TextBox { rect: [40.0, 120.0, 200.0, 160.0], font_size: 12.0 };
    let mut style = pdfcraft_engine::Style::default_for(&shape);
    style.width = 1.0;
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().apply_edit(
        &ctx_h,
        pdfcraft_engine::Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
            page: 0,
            shape,
            style,
            contents: "Pos. 3".into(),
            author: "Tester".into(),
        })
    ));
    h.run_steps(2);
    open_comment_props(&mut h, 0, 0);
    h.run_steps(2);
    h.get_by_label("Border shape");
    {
        let s = h.state();
        assert_eq!(s.session.get(s.views[0].id).unwrap().comment_props(0, 0).unwrap().border_shape, Some(BorderShape::Rectangle));
    }
    set_prop(&mut h, "border_shape", FieldValue::Text(BorderShape::Oval.name().into()));
    set_prop(&mut h, "make_default", FieldValue::Bool(true));
    accept_props(&mut h);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(doc.comment_props(0, 0).unwrap().border_shape, Some(BorderShape::Oval));
    assert_eq!(doc.can_undo(), Some("Change comment properties"));
    assert_eq!(s.comment_prefs.style(CommentTool::TextBox).border_shape, BorderShape::Oval, "the default for new text boxes");
}

/// Thickness 0 in Comment Properties takes a circle away with the line, as Border ▸ None does on
/// the toolbar, and a default made from it draws no circle either.
#[test]
fn no_thickness_in_text_box_properties_takes_the_circle_away() {
    use pdfcraft_engine::BorderShape;
    use pdfcraft_ui_egui::comments::CommentTool;
    let mut h = harness(|_| {});
    let shape = pdfcraft_engine::Shape::TextBox { rect: [40.0, 120.0, 200.0, 160.0], font_size: 12.0 };
    let mut style = pdfcraft_engine::Style::default_for(&shape);
    style.width = 1.0;
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().apply_edit(
        &ctx_h,
        pdfcraft_engine::Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
            page: 0,
            shape,
            style,
            contents: "Pos. 3".into(),
            author: "Tester".into(),
        })
    ));
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().apply_edit(&ctx_h, pdfcraft_engine::Edit::BorderShapeAnnotation { page: 0, index: 0, shape: BorderShape::Circle }));
    h.state_mut().comment_prefs.set_border_shape(CommentTool::TextBox, BorderShape::Circle);
    h.run_steps(2);
    open_comment_props(&mut h, 0, 0);
    h.run_steps(2);
    {
        let s = h.state();
        assert_eq!(s.session.get(s.views[0].id).unwrap().comment_props(0, 0).unwrap().border_shape, Some(BorderShape::Circle));
    }
    set_prop(&mut h, "width", FieldValue::Number(0.0));
    set_prop(&mut h, "make_default", FieldValue::Bool(true));
    accept_props(&mut h);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    let p = doc.comment_props(0, 0).unwrap();
    assert_eq!((p.width, p.border_shape), (Some(0.0), Some(BorderShape::Rectangle)));
    assert_eq!(doc.can_undo(), Some("Change comment properties"));
    assert_eq!(s.comment_prefs.style(CommentTool::TextBox).border_shape, BorderShape::Rectangle, "the default for new text boxes");
}

/// A filled rectangle's line and fill, and a text box's text, border and fill, each take an
/// opacity of their own in Properties; Make Properties Default carries them into the tool, so
/// the next rectangle is drawn with a see-through fill and an opaque line.
#[test]
fn comment_properties_set_each_parts_opacity() {
    use pdfcraft_ui_egui::comments::CommentTool;
    let mut h = harness(|app| app.set_option(&egui::Context::default(), "quick", "square").unwrap());
    h.state_mut().comment_prefs.set_fill(CommentTool::Rectangle, Some([1.0, 1.0, 0.0]));
    h.run_steps(2);
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    let props = |h: &Harness<'static, PdfCraftApp>, index: usize| {
        let s = h.state();
        let p = s.session.get(s.views[0].id).unwrap().comment_props(0, index).unwrap();
        (p.opacity, p.fill_opacity, p.text_opacity)
    };
    open_comment_props(&mut h, 0, 0);
    h.run_steps(2);
    // The sliders (each labelled by its row, which shares the name).
    h.get_by_role_and_label(egui::accesskit::Role::Slider, "Line opacity");
    h.get_by_role_and_label(egui::accesskit::Role::Slider, "Fill opacity");
    assert_eq!(h.query_all_by_label("Text opacity").count(), 0, "a rectangle has no text");
    set_prop(&mut h, "fill_opacity", FieldValue::Number(30.0));
    set_prop(&mut h, "make_default", FieldValue::Bool(true));
    accept_props(&mut h);
    assert_eq!(props(&h, 0), (1.0, Some(0.3), None));
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Change comment properties"));
    assert_eq!(h.state().comment_prefs.style(CommentTool::Rectangle).fill_opacity, Some(0.3));
    h.set_option("quick", "square").unwrap();
    h.run_steps(2);
    drag_pt(&mut h, (160.0, 100.0), (260.0, 40.0));
    assert_eq!(props(&h, 1), (1.0, Some(0.3), None), "the next rectangle takes the default");

    // A text box with a border and a fill: its text, border and fill.
    let shape = pdfcraft_engine::Shape::TextBox { rect: [40.0, 120.0, 200.0, 160.0], font_size: 12.0 };
    let mut style = pdfcraft_engine::Style::default_for(&shape);
    (style.width, style.fill) = (1.0, Some([1.0, 1.0, 0.6]));
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().apply_edit(
        &ctx_h,
        pdfcraft_engine::Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
            page: 0,
            shape,
            style,
            contents: "Note".into(),
            author: "Tester".into(),
        })
    ));
    h.set_option("quick", "select").unwrap();
    h.run_steps(2);
    open_comment_props(&mut h, 0, 2);
    h.run_steps(2);
    for label in ["Text opacity", "Border opacity", "Fill opacity"] {
        h.get_by_role_and_label(egui::accesskit::Role::Slider, label);
    }
    if let Ok(dir) = std::env::var("PDFCRAFT_SHOTS") {
        let _gpu = crate::gpu();
        h.render().unwrap().save(format!("{dir}/comment-part-opacity.png")).unwrap();
    }
    set_prop(&mut h, "opacity", FieldValue::Number(50.0));
    set_prop(&mut h, "fill_opacity", FieldValue::Number(20.0));
    accept_props(&mut h);
    assert_eq!(props(&h, 2), (0.5, Some(0.2), Some(1.0)), "the border at half, the fill at a fifth, the text opaque");
    // Without its fill and border, the text is the whole text box: its opacity is the comment's.
    open_comment_props(&mut h, 0, 2);
    h.run_steps(2);
    clear_prop(&mut h, PROP_FILL);
    set_prop(&mut h, "width", FieldValue::Number(0.0));
    accept_props(&mut h);
    open_comment_props(&mut h, 0, 2);
    h.run_steps(2);
    let shown = egui_kittest::kittest::NodeT::accesskit_node(&h.get_by_role_and_label(egui::accesskit::Role::Slider, "Text opacity")).numeric_value();
    assert_eq!(shown, Some(100.0), "the text's slider shows the opacity it is drawn at, not the border's half");
    set_prop(&mut h, "text_opacity", FieldValue::Number(40.0));
    accept_props(&mut h);
    assert_eq!(props(&h, 2), (0.4, Some(0.4), Some(0.4)));
}

// Contributor-original PDF. Two roots deliberately share /NM; the replies (on another page,
// with dates opposite to source order) point at the first by reference. Nothing replies to the
// unnamed root.
const REVIEW_FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Annots [5 0 R 6 0 R 7 0 R] >> endobj
4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Annots [8 0 R 9 0 R 10 0 R 11 0 R 12 0 R 13 0 R 14 0 R 15 0 R] >> endobj
5 0 obj << /Type /Annot /Subtype /Text /Rect [20 140 40 160] /NM (shared) /T (Alpha) /Contents (First root) >> endobj
6 0 obj << /Type /Annot /Subtype /Text /Rect [60 140 80 160] /NM (shared) /T (Beta) /Contents (Duplicate root) >> endobj
7 0 obj << /Type /Annot /Subtype /Text /Rect [100 140 120 160] /T (Gamma) /Contents (Unnamed root) >> endobj
8 0 obj << /Type /Annot /Subtype /Text /Rect [20 140 40 160] /NM (late) /T (Delta) /Contents (Late root) >> endobj
9 0 obj << /Type /Annot /Subtype /Text /Rect [20 100 40 120] /NM (first-reply) /IRT 5 0 R /T (Reader) /Contents (Needle first reply) /M (D:20260102000000) >> endobj
10 0 obj << /Type /Annot /Subtype /Text /Rect [20 100 40 120] /IRT 5 0 R /StateModel /Review /State /Rejected /Contents (Old review) /M (D:20260102000000) >> endobj
11 0 obj << /Type /Annot /Subtype /Text /Rect [20 100 40 120] /IRT 5 0 R /StateModel /Marked /State /Marked /M (D:20260102000000) >> endobj
12 0 obj << /Type /Annot /Subtype /Text /Rect [20 100 40 120] /NM (second-reply) /IRT 5 0 R /T (Reader) /Contents (Second reply) /M (D:20260101000000) >> endobj
13 0 obj << /Type /Annot /Subtype /Text /Rect [20 100 40 120] /IRT 5 0 R /StateModel /Review /State /Accepted /Contents (Status-only token) /M (D:20260101000000) >> endobj
14 0 obj << /Type /Annot /Subtype /Text /Rect [20 100 40 120] /IRT 5 0 R /StateModel /Marked /State /Unmarked /M (D:20260101000000) >> endobj
15 0 obj << /Type /Annot /Subtype /Text /Rect [20 100 40 120] /NM (late-reply) /IRT 8 0 R /T (Reader) /Contents (Late thread reply) >> endobj
trailer << /Root 1 0 R >>
%%EOF";

#[test]
fn the_panel_preserves_threads_source_order_and_unfiltered_page_counts() {
    let mut h = harness_with_bytes(REVIEW_FIXTURE.to_vec(), |app| {
        app.set_option(&egui::Context::default(), "panel", "comments").unwrap();
        app.views[0].session.comments.panel.search = "needle".into();
        app.views[0].session.comments.panel.search_open = true;
        app.views[0].session.comments.panel.filter.hidden_authors.push("Beta".into());
        app.views[0].session.comments.select_only((0, 0));
    });
    // The panel names a card or reply row by its author, text and date.
    h.get_by_label_contains("First root");
    // All three page roots count, including the two filtered out (F-86: heading count in its name).
    h.get_by_label("Page 1, 3");
    assert_eq!(h.query_all_by_label_contains("Duplicate root").count(), 0);
    assert_eq!(h.query_all_by_label_contains("Unnamed root").count(), 0);
    assert!(
        h.get_by_label_contains("Needle first reply").rect().top() < h.get_by_label_contains("Second reply").rect().top(),
        "replies follow source order, not dates"
    );
    h.get_by_label("Accepted");
    h.get_by_label("Mark with checkmark"); // The later Unmarked reply wins independently.
    assert_eq!(h.query_all_by_label_contains("Status-only token").count(), 0, "status text is searchable but is not a displayed reply");

    h.state_mut().views[0].session.comments.panel.filter.hidden_authors.clear();
    h.run_steps(2);
    // The replies point at the first root: sharing its /NM doesn't give the other its thread
    // (or a search match on that thread).
    assert_eq!(h.query_all_by_label_contains("Duplicate root").count(), 0);
    assert_eq!(h.query_all_by_label_contains("Needle first reply").count(), 1);
    h.state_mut().views[0].session.comments.panel.search = "status-only token".into();
    h.state_mut().views[0].session.comments.panel.filter.hidden_statuses.push(ReviewStatus::Rejected);
    h.state_mut().views[0].session.comments.panel.filter.hidden_checks.push(true);
    h.run_steps(2);
    h.get_by_label_contains("First root");
    assert_eq!(h.query_all_by_label_contains("Duplicate root").count(), 0);

    h.state_mut().views[0].session.comments.panel.filter.hidden_statuses.push(ReviewStatus::Accepted);
    h.run_steps(2);
    h.get_by_label("No comments match.");
    h.state_mut().views[0].session.comments.panel.filter.hidden_statuses.clear();
    h.state_mut().views[0].session.comments.panel.filter.hidden_checks.push(false);
    h.run_steps(2);
    h.get_by_label("No comments match.");

    h.state_mut().views[0].session.comments.panel.search = "unnamed root".into();
    h.state_mut().views[0].session.comments.panel.filter.hidden_checks.clear();
    h.run_steps(2);
    h.get_by_label_contains("Unnamed root");
    assert_eq!(h.query_all_by_label_contains("Needle first reply").count(), 0, "nothing replies to the unnamed root");
}

// Contributor-original PDF. A status is set on a reply (its /IRT names the reply, ISO 32000-2
// §12.5.6.3), not on the comment; the status reply's text is found only there.
const REPLY_STATUS_FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Annots [5 0 R 6 0 R 7 0 R 8 0 R] >> endobj
5 0 obj << /Type /Annot /Subtype /Text /Rect [20 140 40 160] /T (Alpha) /Contents (Plain root) >> endobj
6 0 obj << /Type /Annot /Subtype /Text /Rect [60 140 80 160] /T (Beta) /Contents (Other root) >> endobj
7 0 obj << /Type /Annot /Subtype /Text /Rect [20 100 40 120] /IRT 5 0 R /T (Reader) /Contents (Ordinary reply) >> endobj
8 0 obj << /Type /Annot /Subtype /Text /Rect [20 100 40 120] /IRT 7 0 R /T (Carol) /StateModel /Review /State /Accepted /Contents (Reply-status token) >> endobj
trailer << /Root 1 0 R >>
%%EOF";

#[test]
fn search_finds_a_comment_by_the_status_set_on_one_of_its_replies() {
    let mut h = harness_with_bytes(REPLY_STATUS_FIXTURE.to_vec(), |app| {
        app.set_option(&egui::Context::default(), "panel", "comments").unwrap();
        app.views[0].session.comments.panel.search = "reply-status token".into();
        app.views[0].session.comments.panel.search_open = true;
    });
    h.run_steps(2);
    // The panel names a card or reply row by its author, text and date.
    h.get_by_label_contains("Plain root");
    h.get_by_label_contains("Ordinary reply");
    assert_eq!(h.query_all_by_label_contains("Other root").count(), 0, "the other comment has no such status");
    assert_eq!(h.query_all_by_label("No comments match.").count(), 0);
}

// Contributor-original PDF, shaped like PyMuPDF output: every page's first comment is named
// `fitz-A0`, so only the `/IRT` reference tells which comment a reply belongs to.
const PER_PAGE_NAMES: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Annots [5 0 R] >> endobj
4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Annots [6 0 R 7 0 R 8 0 R] >> endobj
5 0 obj << /Type /Annot /Subtype /Text /Rect [20 140 40 160] /NM (fitz-A0) /T (Alpha) /Contents (Cover note) >> endobj
6 0 obj << /Type /Annot /Subtype /Text /Rect [20 140 40 160] /NM (fitz-A0) /T (Beta) /Contents (Second page note) >> endobj
7 0 obj << /Type /Annot /Subtype /Text /Rect [20 140 40 160] /NM (fitz-A1) /IRT 6 0 R /T (Gamma) /Contents (Reply on page two) >> endobj
8 0 obj << /Type /Annot /Subtype /Text /Rect [20 140 40 160] /NM (fitz-A2) /IRT 6 0 R /T (Beta) /Contents (Answer on page two) >> endobj
trailer << /Root 1 0 R >>
%%EOF";

#[test]
fn replies_follow_irt_references_when_names_repeat_on_every_page() {
    let h = harness_with_bytes(PER_PAGE_NAMES.to_vec(), |app| app.set_option(&egui::Context::default(), "panel", "comments").unwrap());
    // The panel names a card or reply row by its author, text and date.
    let reply = h.get_by_label_contains("Reply on page two");
    assert!(reply.rect().top() > h.get_by_label_contains("Second page note").rect().top(), "the reply sits under page 2's comment");
    assert!(h.get_by_label("Page 2, 1").rect().top() < reply.rect().top(), "not under page 1's comment, which shares the /NM");
    h.get_by_label_contains("Answer on page two");
    // The header counts the two comments, not their replies (Acrobat says 2, not 4), and gives
    // the replies in small text under it (F-86).
    assert!(h.query_all_by_label("Comments").count() >= 1, "the panel title shows");
    assert!(h.query_all_by_label("2").count() >= 1, "the count shows");
    assert_eq!(h.query_all_by_label("Comments, 4").count(), 0);
    h.get_by_label("2 replies");
}

// Contributor-original PDF: a comment whose wrong reply (with a pop-up) comes before it in
// `/Annots` and has an answer of its own, a locked right reply and an Accepted status. `/F 128`
// is the Locked flag.
const REPLY_THREAD: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Annots [5 0 R 9 0 R 4 0 R 6 0 R 7 0 R 8 0 R] >> endobj
4 0 obj << /Type /Annot /Subtype /Text /Rect [20 140 40 160] /NM (note) /T (Ben) /Contents (The comment) >> endobj
5 0 obj << /Type /Annot /Subtype /Text /Rect [20 140 40 160] /NM (wrong) /IRT 4 0 R /T (Cat) /Contents (Wrong reply) /Popup 9 0 R >> endobj
6 0 obj << /Type /Annot /Subtype /Text /Rect [20 140 40 160] /NM (right) /IRT 4 0 R /T (Dan) /Contents (Right reply) /F 128 >> endobj
7 0 obj << /Type /Annot /Subtype /Text /Rect [20 140 40 160] /NM (answer) /IRT 5 0 R /T (Eve) /Contents (Answer to the wrong reply) >> endobj
8 0 obj << /Type /Annot /Subtype /Text /Rect [20 140 40 160] /IRT 4 0 R /StateModel /Review /State /Accepted /T (Ben) /Contents (Accepted set by Ben) >> endobj
9 0 obj << /Type /Annot /Subtype /Popup /Rect [60 100 160 160] /Parent 5 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// Whether `label` names a reply row showing `text`: replies are group nodes in their card, named
/// "{author}: {text}" and the reply's age.
fn names_reply(label: &str, text: &str) -> bool {
    label.split_once(": ").is_some_and(|(_, rest)| rest == text || rest.strip_prefix(text).is_some_and(|age| age.starts_with(", ")))
}

/// The reply rows of the Comments panel.
fn reply_rows<'a>(h: &'a Harness<'static, PdfCraftApp>) -> Vec<egui_kittest::Node<'a>> {
    h.query_all_by(|n| n.role() == Role::Group && n.label().is_some_and(|l| l.contains(": "))).collect()
}

/// The reply row that shows `text`.
fn reply_row<'a>(h: &'a Harness<'static, PdfCraftApp>, text: &str) -> egui_kittest::Node<'a> {
    let wanted = text.to_owned();
    h.query_all_by(move |n| n.role() == Role::Group && n.label().is_some_and(|l| names_reply(&l, &wanted)))
        .next()
        .unwrap_or_else(|| panic!("no reply row shows {text:?}"))
}

/// How many reply rows show `text`.
fn reply_rows_showing(h: &Harness<'static, PdfCraftApp>, text: &str) -> usize {
    let text = text.to_owned();
    h.query_all_by(move |n| n.role() == Role::Group && n.label().is_some_and(|l| names_reply(&l, &text))).count()
}

/// The "…" (More) of the reply row that shows `text`: the one inside that row.
fn reply_options<'a>(h: &'a Harness<'static, PdfCraftApp>, text: &str) -> egui_kittest::Node<'a> {
    let row = reply_row(h, text).rect();
    h.query_all_by_label("More").find(|b| row.contains(b.rect().center())).expect("a reply menu")
}

/// The "…" (More) of a card itself, outside every reply row.
fn card_options<'a>(h: &'a Harness<'static, PdfCraftApp>) -> egui_kittest::Node<'a> {
    let replies: Vec<egui::Rect> = reply_rows(h).iter().map(|r| r.rect()).collect();
    h.query_all_by_label("More").find(|b| !replies.iter().any(|r| r.contains(b.rect().center()))).expect("a card menu")
}

/// How many reply rows have their own "…".
fn reply_menus(h: &Harness<'static, PdfCraftApp>) -> usize {
    let replies: Vec<egui::Rect> = reply_rows(h).iter().map(|r| r.rect()).collect();
    h.query_all_by_label("More").filter(|b| replies.iter().any(|r| r.contains(b.rect().center()))).count()
}

/// Row of `panel_and_card_actions_become_engine_edits`: one reply is deleted from its own "…"
/// menu, with the answer to it and its pop-up: the comment, its other reply and its status stay,
/// and the card stays selected though the comment moved up in `/Annots`. One undo brings the reply
/// back. A locked reply can't be deleted, and Edit changes the reply's text, not the comment's.
fn delete_one_reply_row() {
    let clipboard = support::RecordingClipboard::default();
    let kept = clipboard.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::support::app_with_clipboard(clipboard);
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "text.pdf", None, REPLY_THREAD.to_vec()).expect("opens");
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "author", "Tester").unwrap();
            app.set_option(&egui::Context::default(), "panel", "comments").unwrap();
            app.views[0].session.comments.select_only((0, 2));
            app
        }
    });
    h.run_steps(4);
    crate::support::settle(&mut h);
    let texts = |h: &Harness<'static, PdfCraftApp>| -> Vec<String> { comments(h).iter().filter_map(|a| a.contents.clone()).collect() };
    let all = ["Wrong reply", "The comment", "Right reply", "Answer to the wrong reply", "Accepted set by Ben"];
    assert_eq!(texts(&h), all);
    assert_eq!(reply_menus(&h), 3, "each reply has one; the status is a badge");

    reply_options(&h, "Wrong reply").click();
    h.run_steps(2);
    h.get_by_label("Delete").click();
    h.run_steps(3);
    assert_eq!(texts(&h), ["The comment", "Right reply", "Accepted set by Ben"], "the reply took its answer along");
    let id = h.state().views[0].id;
    assert_eq!(h.state().session.get(id).unwrap().info.annotations[0].index, 0, "its pop-up went too");
    assert_eq!(h.state().views[0].session.comments.selection(), vec![(0, 0)], "the card stays selected where the comment now is");
    assert_eq!(reply_rows_showing(&h, "Wrong reply"), 0);
    assert_eq!(reply_rows_showing(&h, "Answer to the wrong reply"), 0);
    reply_row(&h, "Right reply");
    h.get_by_label("Ben: Accepted"); // One badge per reviewer.
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Delete comment"), "one undo step");

    // Copy text copies the reply's text.
    reply_options(&h, "Right reply").click();
    h.run_steps(2);
    h.get_by_label("Copy text").click();
    h.run_steps(4);
    assert_eq!(kept.copied().as_deref(), Some("Right reply"), "the reply's text is copied");
    h.run_steps(2);

    // The locked reply's Delete is greyed out; its text can still be edited (ISO 32000-2
    // Table 167: Locked doesn't restrict the contents).
    reply_options(&h, "Right reply").click();
    h.run_steps(2);
    assert!(h.get_by_label("Delete").accesskit_node().is_disabled(), "a locked reply isn't deleted");
    h.query_all_by_label("Edit").last().expect("the reply menu's Edit, after the toolbar tab").click();
    h.run_steps(2);
    let editor = h
        .query_all_by(|n| n.role() == Role::MultilineTextInput && !n.is_hidden() && n.label().as_deref() == Some("Edit"))
        .next()
        .expect("reply editor");
    editor.click();
    h.run_steps(1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.event(egui::Event::Text("Right reply, reworded".into()));
    h.run_steps(2);
    // The document's own Save sits earlier in the tree than the editor's.
    h.query_all_by_label("Save").filter(|n| !n.accesskit_node().is_disabled()).last().expect("the editor's Save").click();
    h.run_steps(3);
    assert_eq!(texts(&h), ["The comment", "Right reply, reworded", "Accepted set by Ben"]);

    // The locked reply keeps its comment too: deleting the comment would take it along.
    card_options(&h).click();
    h.run_steps(2);
    assert!(h.get_by_label("Delete").accesskit_node().is_disabled(), "the comment of a locked reply isn't deleted");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);

    undo(&mut h);
    h.run_steps(2);
    undo(&mut h);
    h.run_steps(3);
    assert_eq!(texts(&h), all, "undo brings the reply and its answer back");
    reply_row(&h, "Wrong reply");
}

// Contributor-original PDF: a comment and its replies A, B and C, in that `/Annots` order.
const THREE_REPLIES: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Annots [4 0 R 5 0 R 6 0 R 7 0 R] >> endobj
4 0 obj << /Type /Annot /Subtype /Text /Rect [20 140 40 160] /NM (note) /T (Ben) /Contents (The comment) >> endobj
5 0 obj << /Type /Annot /Subtype /Text /Rect [20 140 40 160] /NM (a) /IRT 4 0 R /T (Cat) /Contents (Reply A) >> endobj
6 0 obj << /Type /Annot /Subtype /Text /Rect [20 140 40 160] /NM (b) /IRT 4 0 R /T (Dan) /Contents (Reply B) >> endobj
7 0 obj << /Type /Annot /Subtype /Text /Rect [20 140 40 160] /NM (c) /IRT 4 0 R /T (Eve) /Contents (Reply C) >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// Opens the inline editor on the reply that shows `text`, from its "…" menu, and types `draft`
/// into it without saving.
fn draft_reply(h: &mut Harness<'static, PdfCraftApp>, text: &str, draft: &str) {
    reply_options(h, text).click();
    h.run_steps(2);
    h.query_all_by_label("Edit").last().expect("the reply menu's Edit, after the toolbar tab").click();
    h.run_steps(2);
    let editor = h
        .query_all_by(|n| n.role() == Role::MultilineTextInput && !n.is_hidden() && n.label().as_deref() == Some("Edit"))
        .next()
        .expect("reply editor");
    editor.click();
    h.run_steps(1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.event(egui::Event::Text(draft.into()));
    h.run_steps(2);
}

/// A reply's inline editor, left open with a draft, closes on Undo and on Redo. Either can put
/// a reply back in its place or take one away, and the editor would have saved the draft over
/// whichever reply took that place: reply A after delete → Undo, reply C after Redo.
#[test]
fn undo_and_redo_close_a_reply_editor_left_open() {
    let mut h = harness_with_bytes(THREE_REPLIES.to_vec(), |app| {
        app.set_option(&egui::Context::default(), "panel", "comments").unwrap();
        app.views[0].session.comments.select_only((0, 0));
    });
    let texts = |h: &Harness<'static, PdfCraftApp>| -> Vec<String> { comments(h).iter().filter_map(|a| a.contents.clone()).collect() };
    let all = ["The comment", "Reply A", "Reply B", "Reply C"];
    assert_eq!(texts(&h), all);

    draft_reply(&mut h, "Reply B", "B's draft");
    reply_options(&h, "Reply A").click();
    h.run_steps(2);
    h.get_by_label("Delete").click();
    h.run_steps(3);
    assert_eq!(texts(&h), ["The comment", "Reply B", "Reply C"]);
    assert_eq!(note(&h).as_deref(), Some("B's draft"), "the editor follows B up");
    assert_eq!(h.state().views[0].session.comments.panel.editing.as_ref().map(|e| e.1), Some(1), "to the reply that took B's place");

    undo(&mut h);
    h.run_steps(3);
    assert_eq!(texts(&h), all, "undo puts A back where B's editor now points");
    assert_eq!(note(&h), None, "the editor closes on undo");
    let edit_fields = |h: &Harness<'static, PdfCraftApp>| {
        h.query_all_by(|n| n.role() == Role::MultilineTextInput && !n.is_hidden() && n.label().as_deref() == Some("Edit")).count()
    };
    assert_eq!(edit_fields(&h), 0, "no draft is left to save over A");
    assert_eq!(h.query_all_by_label("Cancel").count(), 0, "the inline edit controls closed");

    h.state_mut().views[0].session.comments.select_only((0, 0));
    h.run_steps(2);
    draft_reply(&mut h, "Reply B", "B's draft");
    assert_eq!(note(&h).as_deref(), Some("B's draft"));
    assert_eq!(h.state().views[0].session.comments.panel.editing.as_ref().map(|e| e.1), Some(2));
    redo(&mut h);
    h.run_steps(3);
    assert_eq!(texts(&h), ["The comment", "Reply B", "Reply C"], "redo deletes A again, and C takes B's place");
    assert_eq!(note(&h), None, "the editor closes on redo");
    assert_eq!(edit_fields(&h), 0, "no draft is left to save over C");
    assert_eq!(h.query_all_by_label("Cancel").count(), 0, "the inline edit controls closed");
}

// Contributor-original PDF. Page 1: an unnamed comment with a reply and a reply to that reply,
// a Replace Text strikeout with its grouped caret, and a hidden status whose comment no page
// lists. Page 2: an `/IRT` to the annotation itself, to an annotation no page lists (object 11),
// to a pop-up, to a link and to a form field, and two annotations whose `/IRT`s point at each other.
const THREAD_EDGES: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Annots [5 0 R 6 0 R 7 0 R 8 0 R 9 0 R 19 0 R] >> endobj
4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Annots [10 0 R 12 0 R 13 0 R 14 0 R 15 0 R 16 0 R 17 0 R 18 0 R 20 0 R 21 0 R] >> endobj
5 0 obj << /Type /Annot /Subtype /Text /Rect [20 160 40 180] /T (Alpha) /Contents (Unnamed parent) >> endobj
6 0 obj << /Type /Annot /Subtype /Text /Rect [20 160 40 180] /IRT 5 0 R /T (Beta) /Contents (Direct reply) >> endobj
7 0 obj << /Type /Annot /Subtype /Text /Rect [20 160 40 180] /IRT 6 0 R /T (Gamma) /Contents (Reply to the reply) >> endobj
8 0 obj << /Type /Annot /Subtype /StrikeOut /Rect [60 100 160 112] /IT /StrikeOutTextEdit /T (Alpha) /Contents (Struck words) >> endobj
9 0 obj << /Type /Annot /Subtype /Caret /Rect [158 96 166 104] /IRT 8 0 R /RT /Group /T (Alpha) /Contents (Replacement words) >> endobj
10 0 obj << /Type /Annot /Subtype /Text /Rect [20 160 40 180] /IRT 10 0 R /T (Delta) /Contents (Points at itself) >> endobj
11 0 obj << /Type /Annot /Subtype /Text /Rect [20 120 40 140] /T (Nobody) /Contents (Listed on no page) >> endobj
12 0 obj << /Type /Annot /Subtype /Text /Rect [20 120 40 140] /IRT 11 0 R /T (Delta) /Contents (Points at an unlisted note) >> endobj
13 0 obj << /Type /Annot /Subtype /Popup /Rect [120 100 280 180] >> endobj
14 0 obj << /Type /Annot /Subtype /Text /Rect [20 80 40 100] /IRT 13 0 R /T (Delta) /Contents (Points at a pop-up) >> endobj
15 0 obj << /Type /Annot /Subtype /Link /Rect [200 20 260 40] /Border [0 0 0] /Dest [3 0 R /Fit] >> endobj
16 0 obj << /Type /Annot /Subtype /Widget /FT /Tx /T (field) /Rect [200 60 260 80] >> endobj
17 0 obj << /Type /Annot /Subtype /Text /Rect [20 50 40 70] /IRT 15 0 R /T (Delta) /Contents (Points at a link) >> endobj
18 0 obj << /Type /Annot /Subtype /Text /Rect [20 20 40 40] /IRT 16 0 R /T (Delta) /Contents (Points at a form field) >> endobj
19 0 obj << /Type /Annot /Subtype /Text /Rect [240 20 260 40] /F 30 /IRT 11 0 R /T (Delta) /StateModel /Review /State /Accepted /Contents (Stray status) >> endobj
20 0 obj << /Type /Annot /Subtype /Text /Rect [60 20 80 40] /IRT 21 0 R /T (Delta) /Contents (Loop one) >> endobj
21 0 obj << /Type /Annot /Subtype /Text /Rect [90 20 110 40] /IRT 20 0 R /T (Delta) /Contents (Loop two) >> endobj
trailer << /Root 1 0 R >>
%%EOF";

#[test]
fn replies_that_lead_nowhere_stay_comments_and_deeper_replies_join_their_thread() {
    let mut h = harness_with_size(THREAD_EDGES.to_vec(), egui::vec2(1400.0, 2000.0), |app| {
        app.set_option(&egui::Context::default(), "panel", "comments").unwrap();
        app.set_option(&egui::Context::default(), "quick", "select").unwrap();
    });
    // An `/IRT` that reaches no listed comment leaves its annotation a comment of its own,
    // rather than a reply shown nowhere.
    // Links, form widgets and pop-ups are never parents: neither reader lists them as comments.
    for orphan in
        ["Points at itself", "Points at an unlisted note", "Points at a pop-up", "Points at a link", "Points at a form field", "Loop one", "Loop two"]
    {
        // The panel names a card or reply row by its author, text and date.
        assert!(h.get_by_label_contains(orphan).rect().top() > h.get_by_label("Page 2, 7").rect().top(), "{orphan}");
    }
    // A reply to a reply joins the thread of the comment it starts from, also when that comment
    // has no /NM.
    let (parent, direct, deeper) =
        (h.get_by_label_contains("Unnamed parent"), h.get_by_label_contains("Direct reply"), h.get_by_label_contains("Reply to the reply"));
    assert!(parent.rect().top() < direct.rect().top() && direct.rect().top() < deeper.rect().top());
    assert!(deeper.rect().top() < h.get_by_label_contains("Struck words").rect().top(), "under its own thread");
    // Comments: the unnamed parent, the strikeout and the seven on page 2. Replies: the two
    // replies, not the Replace Text caret grouped with its strikeout (F-86).
    assert!(h.query_all_by_label("Comments").count() >= 1, "the panel title shows");
    assert!(h.query_all_by_label("9").count() >= 1, "the count shows");
    h.get_by_label("2 replies");
    // A status whose comment no page lists is not a comment: no card, and nothing to pick on the
    // page where it sits.
    assert_eq!(h.query_all_by_label_contains("Stray status").count(), 0);
    click_pt(&mut h, (250.0, 30.0));
    assert!(h.state().views[0].session.comments.selection().is_empty());
}

/// The arrow keys move a comment, not a reply: also not a reply to a comment with no `/NM`.
#[test]
fn arrow_keys_leave_a_reply_to_an_unnamed_comment_where_it_is() {
    let mut h = harness_with_bytes(THREAD_EDGES.to_vec(), |app| app.set_option(&egui::Context::default(), "quick", "select").unwrap());
    let rect = |h: &Harness<'static, PdfCraftApp>, text: &str| comments(h).into_iter().find(|a| a.contents.as_deref() == Some(text)).unwrap().rect;
    let (reply, parent) = (rect(&h, "Direct reply"), rect(&h, "Unnamed parent"));
    h.state_mut().views[0].session.comments.select_only((0, 1));
    h.key_press(egui::Key::ArrowRight);
    h.run_steps(3);
    assert_eq!(rect(&h, "Direct reply"), reply, "the reply stays");
    h.state_mut().views[0].session.comments.select_only((0, 0));
    h.key_press(egui::Key::ArrowRight);
    h.run_steps(3);
    assert_ne!(rect(&h, "Unnamed parent"), parent, "its comment moves");
}

// Contributor-original PDF: a comment, a reply, and a checkmark and a status set on the reply.
const CHECKED_REPLY: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Annots [4 0 R 5 0 R 6 0 R 7 0 R] >> endobj
4 0 obj << /Type /Annot /Subtype /Text /Rect [100 100 120 120] /T (Alpha) /Contents (Unchecked comment) >> endobj
5 0 obj << /Type /Annot /Subtype /Text /Rect [100 60 120 80] /IRT 4 0 R /T (Beta) /Contents (Checked reply) >> endobj
6 0 obj << /Type /Annot /Subtype /Text /Rect [100 60 120 80] /IRT 5 0 R /T (Beta) /StateModel /Marked /State /Marked >> endobj
7 0 obj << /Type /Annot /Subtype /Text /Rect [100 60 120 80] /IRT 5 0 R /T (Beta) /StateModel /Review /State /Accepted >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// A checkmark or status belongs to the annotation its `/IRT` names (ISO 32000-2 §12.5.6.2): one
/// set on a reply doesn't mark the comment, on its card or in the page's menu.
#[test]
fn a_checkmark_on_a_reply_does_not_mark_its_comment() {
    let mut h = harness_with_bytes(CHECKED_REPLY.to_vec(), |app| {
        app.set_option(&egui::Context::default(), "panel", "comments").unwrap();
        app.set_option(&egui::Context::default(), "quick", "select").unwrap();
    });
    click_pt(&mut h, (110.0, 110.0));
    assert_eq!(h.state().views[0].session.comments.selection(), vec![(0, 0)], "the comment is selected");
    reply_row(&h, "Checked reply");
    h.get_by_label("Mark with checkmark"); // The card's checkmark.
    assert_eq!(h.query_all_by_label("Accepted").count(), 0, "no status badge");
    let p = at(&h, 110.0, 110.0);
    h.hover_at(p);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed: true, modifiers: Default::default() });
    h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed: false, modifiers: Default::default() });
    h.run_steps(3);
    h.get_by_label("Copy text"); // The page's menu is open.
    assert_eq!(h.query_all_by_label("Remove checkmark").count(), 0);
    assert_eq!(h.query_all_by_label("Mark with checkmark").count(), 2, "the card and the page's menu");
}

#[test]
fn the_panel_reveals_edits_and_reopens_a_late_thread_without_changing_its_replies() {
    let mut h = harness_with_bytes(REVIEW_FIXTURE.to_vec(), |app| {
        app.set_option(&egui::Context::default(), "panel", "comments").unwrap();
        app.views[0].session.comments.panel.collapsed_pages = vec![0, 1];
    });
    // The panel names a card or reply row by its author, text and date.
    assert_eq!(h.query_all_by_label_contains("Late root").count(), 0);
    h.state_mut().views[0].session.comments.select_only((1, 0));
    h.state_mut().views[0].session.comments.reveal = true;
    h.run_steps(4);
    h.get_by_label_contains("Late root");
    h.get_by_label_contains("Late thread reply");
    assert_eq!(h.state().views[0].session.comments.panel.collapsed_pages, vec![0]);
    card_options(&h).click();
    h.run_steps(2);
    h.query_all_by_label("Edit").last().expect("comment menu edit, after the toolbar tab").click();
    h.run_steps(2);
    let editor = h
        .query_all_by(|n| n.role() == egui::accesskit::Role::MultilineTextInput && !n.is_hidden() && n.label().as_deref() == Some("Edit"))
        .next()
        .expect("note editor");
    let editor_bottom = editor.rect().bottom();
    editor.click();
    h.run_steps(1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.event(egui::Event::Text("Late root edited exactly".into()));
    h.run_steps(2);
    // The Comments panel's inline editor commits with its own Save button right beneath the Edit field.
    h.query_all_by_role_and_label(Role::Button, "Save")
        .filter(|n| n.rect().top() >= editor_bottom - 2.0)
        .min_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .expect("inline edit Post button")
        .click();
    h.run_steps(3);
    assert_eq!(h.state().views[0].session.comments.selection(), vec![(1, 0)]);
    let before = comments(&h);
    let late = before.iter().find(|a| a.name.as_deref() == Some("late")).unwrap();
    assert_eq!(late.contents.as_deref(), Some("Late root edited exactly"));
    let id = h.state().views[0].id;
    let saved = h.state().session.save_bytes(id).unwrap().as_ref().clone();
    let mut reopened = harness_with_bytes(saved, |app| {
        app.set_option(&egui::Context::default(), "panel", "comments").unwrap();
        app.views[0].session.comments.panel.search = "late thread reply".into();
        app.views[0].session.comments.panel.search_open = true;
    });
    // The panel names a card by its author, text and date.
    h.get_by_label_contains("Late root edited exactly");
    reopened.get_by_label_contains("Late root edited exactly");
    reply_row(&reopened, "Late thread reply");
    assert_eq!(reopened.query_all_by_label_contains("First root").count(), 0);
    let after = comments(&reopened);
    assert_eq!(after.len(), before.len());
    for (original, saved) in before.iter().zip(&after) {
        assert_eq!(
            (&saved.name, &saved.contents, &saved.in_reply_to, &saved.state),
            (&original.name, &original.contents, &original.in_reply_to, &original.state)
        );
    }
    assert_eq!(after.iter().find(|a| a.name.as_deref() == Some("late-reply")).unwrap().in_reply_to.as_deref(), Some("late"));
    reopened.state_mut().views[0].session.comments.panel.sort = CommentSort::Author;
    reopened.run_steps(2);
    reopened.get_by_label("Delta, 1"); // The author group (F-86: heading includes count).
    reopened.get_by_label_contains("Delta: Late root edited exactly"); // And its card, still shown after sorting.
}

/// A page with one rectangle comment ("Reviewed"), the Comments panel open and nothing selected.
fn flatten_harness() -> Harness<'static, PdfCraftApp> {
    let mut h = harness(|app| {
        app.set_option(&egui::Context::default(), "panel", "comments").unwrap();
        app.set_option(&egui::Context::default(), "quick", "select").unwrap();
    });
    let shape = pdfcraft_engine::Shape::Rectangle { rect: [20.0, 20.0, 80.0, 60.0] };
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(
        &ctx_h,
        pdfcraft_engine::Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
            page: 0,
            style: pdfcraft_engine::Style::default_for(&shape),
            shape,
            contents: "Reviewed".into(),
            author: "Tester".into(),
        }),
    );
    h.state_mut().views[0].session.comments.clear_selection();
    crate::support::settle(&mut h);
    assert_eq!(comments(&h).len(), 1);
    h
}

/// Right-click the page at a point (user space) and settle.
fn right_click_pt(h: &mut Harness<'static, PdfCraftApp>, p: (f32, f32)) {
    let p = at(h, p.0, p.1);
    h.hover_at(p);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed: true, modifiers: Default::default() });
    h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed: false, modifiers: Default::default() });
    h.run_steps(3);
}

/// Row of `panel_and_card_actions_become_engine_edits`: the panel's "…" menu flattens the
/// comments into the page, and one undo step brings them back.
fn flatten_from_the_panel_menu_row() {
    let mut h = flatten_harness();
    h.get_by_label("More options").click();
    h.run_steps(2);
    h.get_by_label("Flatten comments").click();
    h.run_steps(4);
    assert!(comments(&h).is_empty(), "the panel's menu flattens the comments into the page");
    undo(&mut h);
    h.run_steps(3);
    assert_eq!(comments(&h).len(), 1, "one undo step brings it back");
}

#[test]
fn comments_take_checkmarks_lock_hide_and_summarize() {
    let (mut h, clipboard) = harness_copying(|app| {
        app.set_option(&egui::Context::default(), "panel", "comments").unwrap();
        app.set_option(&egui::Context::default(), "quick", "square").unwrap();
    });
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    h.set_option("quick", "select").unwrap();
    h.run_steps(2);
    assert_eq!(h.state().views[0].session.comments.selection(), vec![(0, 0)], "the new comment is selected");
    // The card's checkmark.
    h.get_by_label("Mark with checkmark").click();
    h.run_steps(3);
    assert!(comments(&h).iter().any(|a| a.state.as_deref() == Some("Marked")));
    h.get_by_role_and_label(Role::Button, "Remove checkmark");

    // "…" ▸ Copy text puts the comment's text on the clipboard.
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(&ctx_h, pdfcraft_engine::Edit::SetAnnotationContents { page: 0, index: 0, text: "Copy me".into() });
    h.run_steps(2);
    h.get_by_label("More").click();
    h.run_steps(2);
    h.get_by_label("Copy text").click();
    h.run_steps(4);
    assert_eq!(clipboard.copied().as_deref(), Some("Copy me"), "the text is copied");

    // Properties ▸ Locked.
    open_comment_props(&mut h, 0, 0);
    h.run_steps(2);
    set_prop(&mut h, "locked", FieldValue::Bool(true));
    accept_props(&mut h);
    assert!(comments(&h)[0].locked);
    h.key_press(egui::Key::Delete);
    h.run_steps(3);
    assert!(comments(&h).iter().any(|a| a.in_reply_to.is_none()), "a locked comment isn't deleted");

    // "…" ▸ Hide all comments, then Show all comments.
    h.get_by_label("More options").click();
    h.run_steps(2);
    h.get_by_label("Hide all comments").click();
    h.run_steps(2);
    let s = h.state();
    assert!(s.session.get(s.views[0].id).unwrap().comments_hidden());
    h.get_by_label("More options").click();
    h.run_steps(2);
    h.get_by_label("Show all comments").click();
    h.run_steps(2);
    let s = h.state();
    assert!(!s.session.get(s.views[0].id).unwrap().comments_hidden());

    // "…" ▸ Create PDF comment summary.
    h.get_by_label("More options").click();
    h.run_steps(2);
    h.get_by_label("Create PDF comment summary…").click();
    h.run_steps(2);
    h.get_by_role_and_label(Role::Dialog, "Summarize Options");
    h.get_by_label("Create PDF Comment Summary").click();
    h.run_steps(3);
    let s = h.state();
    assert_eq!(s.views.len(), 2);
    assert!(s.session.get(s.views[1].id).unwrap().name.starts_with("Summary of comments on text"));
}

#[test]
fn polygons_clouds_connected_lines_callouts_and_inserted_text() {
    let mut h = visible_page_harness(|app| app.set_option(&egui::Context::default(), "quick", "polygon").unwrap());
    // Polygon: click the corners, Enter finishes.
    for p in [(40.0, 40.0), (120.0, 40.0), (80.0, 110.0)] {
        click_pt(&mut h, p);
    }
    assert_eq!(comments(&h).len(), 0, "still drawing");
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    let c = comments(&h);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].subtype, "Polygon");

    // Cloud: clicking the first point again closes it.
    h.set_option("quick", "cloud").unwrap();
    for p in [(160.0, 40.0), (260.0, 40.0), (260.0, 100.0), (160.0, 100.0), (160.0, 40.0)] {
        click_pt(&mut h, p);
    }
    assert_eq!(comments(&h).len(), 2);

    // Connected lines.
    h.set_option("quick", "polyline").unwrap();
    for p in [(20.0, 150.0), (60.0, 130.0), (100.0, 150.0)] {
        click_pt(&mut h, p);
    }
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert!(comments(&h).iter().any(|a| a.subtype == "PolyLine"));

    // Callout: drag from the target to where the box goes, then type.
    h.set_option("quick", "callout").unwrap();
    drag_pt(&mut h, (60.0, 160.0), (180.0, 190.0));
    assert!(h.state().views[0].session.comments.composer.is_some(), "the composer opened");
    field(&h, "Type text").type_text("Why?");
    h.run_steps(2);
    h.get_by_label("Post").click();
    h.run_steps(3);
    let callout = comments(&h).into_iter().find(|a| a.subtype == "FreeText").expect("a callout");
    assert_eq!(callout.contents.as_deref(), Some("Why?"));
    assert!(callout.rect[0] <= 60.0 && callout.rect[1] <= 160.0, "holds the leader: {:?}", callout.rect);
    // Its knee line is Acrobat's: level, 12 pt out from the middle of the box's left side.
    let (tb, knee) = (callout.text_box.expect("a text box"), callout.points[1]);
    assert!((knee[0] - (tb[0] - 12.0)).abs() < 0.01 && (knee[1] - (tb[1] + tb[3]) / 2.0).abs() < 0.01, "{tb:?} {knee:?}");

    // Insert text at a click.
    h.set_option("quick", "caret").unwrap();
    click_pt(&mut h, (95.0, 150.0));
    field(&h, "Text to insert").type_text("very ");
    h.run_steps(2);
    h.get_by_label("Post").click();
    h.run_steps(3);
    let caret = comments(&h).into_iter().find(|a| a.subtype == "Caret").expect("a caret");
    assert_eq!(caret.contents.as_deref(), Some("very"));
    assert_eq!(h.state().quick_tool, QuickTool::Select);
}

fn drawn_points(h: &Harness<'static, PdfCraftApp>) -> usize {
    h.state().views[0].session.comments.tool.drawing().len()
}

#[test]
fn a_wrong_polygon_point_is_taken_back_with_backspace_or_undo() {
    let mut h = harness(|app| app.set_option(&egui::Context::default(), "quick", "polygon").unwrap());
    for p in [(40.0, 40.0), (120.0, 40.0), (200.0, 150.0)] {
        click_pt(&mut h, p);
    }
    assert_eq!(drawn_points(&h), 3);
    // Backspace takes back the wrong click, then Undo takes back the one before it.
    h.key_press(egui::Key::Backspace);
    h.run_steps(2);
    assert_eq!(drawn_points(&h), 2);
    undo(&mut h);
    h.run_steps(2);
    assert_eq!(drawn_points(&h), 1);
    // The right corner goes where the wrong one was, and the polygon finishes with all three.
    click_pt(&mut h, (80.0, 110.0));
    click_pt(&mut h, (120.0, 40.0));
    assert_eq!(drawn_points(&h), 3);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    let c = comments(&h);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].subtype, "Polygon");
    // Taking back the only point cancels the drawing without making a comment.
    h.set_option("quick", "polygon").unwrap();
    click_pt(&mut h, (200.0, 200.0));
    h.key_press(egui::Key::Backspace);
    h.run_steps(2);
    assert_eq!(drawn_points(&h), 0);
    assert!(h.state().views[0].session.comments.tool.drawing().is_empty());
    assert_eq!(comments(&h).len(), 1);
}

#[test]
fn inserted_text_snaps_to_the_gap_between_characters_on_the_baseline() {
    let mut h = harness(|app| app.set_option(&egui::Context::default(), "quick", "caret").unwrap());
    // "The quick brown fox", Helvetica 14 pt from x = 20 on the baseline y = 150. With the
    // standard-14 widths "The quick b" ends at 92.366 and "r" (333/1000 em) at 97.028, so a click
    // in the right half of "r", above the baseline, puts the tip between "r" and "o".
    click_pt(&mut h, (95.5, 155.0));
    field(&h, "Text to insert").type_text("x");
    h.run_steps(2);
    h.get_by_label("Post").click();
    h.run_steps(3);
    let caret = comments(&h).into_iter().find(|a| a.subtype == "Caret").expect("a caret");
    let [x0, y0, x1, y1] = caret.rect;
    assert!(((x0 + x1) / 2.0 - 97.03).abs() < 0.3, "tip at the r|o gap: {:?}", caret.rect);
    assert!((y1 - 150.0).abs() < 0.3, "tip on the baseline: {:?}", caret.rect);
    assert!((y1 - y0 - 7.0).abs() < 0.3, "half the 14 pt em: {:?}", caret.rect);

    // Away from the text the caret stays where the click was, at its default size.
    h.set_option("quick", "caret").unwrap();
    click_pt(&mut h, (200.0, 60.0));
    field(&h, "Text to insert").type_text("y");
    h.run_steps(2);
    h.get_by_label("Post").click();
    h.run_steps(3);
    let away = comments(&h).into_iter().find(|a| a.subtype == "Caret" && a.contents.as_deref() == Some("y")).expect("a second caret");
    let [x0, y0, x1, y1] = away.rect;
    assert!(((x0 + x1) / 2.0 - 200.0).abs() < 1.0 && (y1 - 60.0).abs() < 1.0 && (y1 - y0 - 8.0).abs() < 0.3, "{:?}", away.rect);
}

/// ISO 32000-2 §12.5.6.3 and Table 174: a comment shows each reviewer's latest status, with
/// their name, and a status set back to None stays shown; a checkmark shows only for the
/// reviewer who set it.
#[test]
fn the_panel_shows_each_reviewers_status_and_only_my_checkmark() {
    let mut h = harness(|app| {
        app.set_option(&egui::Context::default(), "panel", "comments").unwrap();
        app.set_option(&egui::Context::default(), "quick", "square").unwrap();
    });
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    h.set_option("quick", "select").unwrap();
    let me = h.state().comment_prefs.author.clone();
    let other = format!("{me} (other)");
    for (state, author) in [
        (pdfcraft_engine::ReviewState::Accepted, "Bob"),
        (pdfcraft_engine::ReviewState::Completed, "Carol"),
        (pdfcraft_engine::ReviewState::Rejected, "Bob"),
        (pdfcraft_engine::ReviewState::Completed, "Dan"),
        (pdfcraft_engine::ReviewState::None, "Dan"),
    ] {
        let ctx_h = h.ctx.clone();
        h.state_mut().apply_edit(&ctx_h, pdfcraft_engine::Edit::SetAnnotationStatus { page: 0, index: 0, state, author: author.into() });
    }
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(&ctx_h, pdfcraft_engine::Edit::MarkAnnotation { page: 0, index: 0, marked: true, author: other.clone() });
    h.state_mut().views[0].session.comments.select_only((0, 0));
    h.run_steps(2);
    h.get_by_label("Bob: Rejected");
    h.get_by_label("Carol: Completed");
    h.get_by_label("Dan: None");
    assert_eq!(h.query_all_by_label("Bob: Accepted").count(), 0, "only Bob's latest");
    assert!(h.query_all_by_label("Remove checkmark").next().is_none(), "another reviewer's checkmark isn't mine");
    h.get_by_label("Mark with checkmark");

    // Filters: a comment stays while any reviewer's status is shown; one nobody set a status on
    // counts as None; the checkmark filter goes by my checkmark only.
    h.set_option("quick", "circle").unwrap();
    drag_pt(&mut h, (160.0, 100.0), (260.0, 40.0));
    h.set_option("quick", "select").unwrap();
    h.state_mut().views[0].session.comments.clear_selection();
    // The panel names a card by its author and text: each shape gets a text to tell its card by.
    let oval = comments(&h).iter().find(|a| a.subtype == "Circle").map(|a| a.index).expect("the oval");
    for (index, text) in [(0, "The rectangle"), (oval, "The oval")] {
        let ctx_h = h.ctx.clone();
        h.state_mut().apply_edit(&ctx_h, pdfcraft_engine::Edit::SetAnnotationContents { page: 0, index, text: text.into() });
    }
    h.run_steps(2);
    let cards = |h: &Harness<'static, PdfCraftApp>| {
        (h.query_all_by_label_contains("The rectangle").count(), h.query_all_by_label_contains("The oval").count())
    };
    assert_eq!(cards(&h), (1, 1), "both cards show");
    let shown = cards(&h);
    h.state_mut().views[0].session.comments.panel.filter.hidden_statuses = vec![ReviewStatus::Rejected];
    h.run_steps(2);
    assert_eq!(cards(&h), shown, "Carol's and Dan's statuses still show the rectangle");
    h.state_mut().views[0].session.comments.panel.filter.hidden_statuses = vec![ReviewStatus::None];
    h.run_steps(2);
    assert_eq!(cards(&h), (shown.0, shown.1 - 1), "the oval has no status, so it counts as None");
    h.state_mut().views[0].session.comments.panel.filter.hidden_statuses = vec![ReviewStatus::Rejected, ReviewStatus::Completed, ReviewStatus::None];
    h.run_steps(2);
    assert_eq!(cards(&h), (shown.0 - 1, shown.1 - 1), "every status hidden");
    h.state_mut().views[0].session.comments.panel.filter.hidden_statuses.clear();
    h.state_mut().views[0].session.comments.panel.filter.hidden_checks = vec![true];
    h.run_steps(2);
    assert_eq!(cards(&h), shown, "another reviewer's checkmark isn't mine");

    h.state_mut().views[0].session.comments.panel.filter.hidden_checks.clear();
    h.state_mut().views[0].session.comments.select_only((0, 0));
    h.state_mut().comment_prefs.author = other;
    h.run_steps(2);
    h.get_by_label("Remove checkmark");
}

#[test]
fn make_current_properties_default() {
    let mut h = harness(|app| {
        app.set_option(&egui::Context::default(), "panel", "comments").unwrap();
        app.set_option(&egui::Context::default(), "quick", "square").unwrap();
    });
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    h.set_option("quick", "select").unwrap();
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(
        &ctx_h,
        pdfcraft_engine::Edit::StyleAnnotation {
            page: 0,
            index: 0,
            color: Some([0.0, 0.47, 0.84]),
            opacity: Some(0.5),
            width: Some(5.0),
            endings: None,
        },
    );
    h.run_steps(2);
    h.get_by_label("More").click();
    h.run_steps(2);
    h.get_by_label("Make Current Properties Default").click();
    h.run_steps(3);
    let st = h.state().comment_prefs.style(pdfcraft_ui_egui::comments::CommentTool::Rectangle);
    assert_eq!((st.color, st.opacity, st.width), ([0.0, 0.47, 0.84], 0.5, 5.0));
    // The default survives a restart (#340): a fresh app reads the persisted settings and its
    // next rectangle still takes the style.
    let saved = h.state().settings_snapshot();
    let mut fresh = crate::control::new_app();
    fresh.apply_settings(&saved);
    let st = fresh.comment_prefs.style(pdfcraft_ui_egui::comments::CommentTool::Rectangle);
    assert_eq!((st.color, st.opacity, st.width), ([0.0, 0.47, 0.84], 0.5, 5.0), "the tool default persisted");
    // The next rectangle takes it.
    h.set_option("quick", "square").unwrap();
    drag_pt(&mut h, (160.0, 100.0), (260.0, 40.0));
    let c = comments(&h);
    let last = c.iter().rfind(|a| a.subtype == "Square").unwrap();
    assert_eq!(last.color, Some([0.0, 0.47, 0.84]));
}

/// Acrobat's Properties ▸ Make Properties Default: OK with the box ticked makes the edited look
/// the tool's default (here a strikethrough's colour); without it, only the comment changes.
#[test]
fn properties_dialog_makes_the_strikethrough_colour_default() {
    use pdfcraft_ui_egui::comments::CommentTool;
    let mut h = harness(|app| app.set_option(&egui::Context::default(), "quick", "strikeout").unwrap());
    let green = [0.18, 0.62, 0.36];
    let blue = [0.0, 0.47, 0.84];
    assert_ne!(h.state().comment_prefs.style(CommentTool::StrikeOut).color, green);
    drag_pt(&mut h, (18.0, 155.0), (150.0, 155.0));
    assert_eq!(comments(&h).iter().filter(|a| a.subtype == "StrikeOut").count(), 1);

    // Without the box: the comment turns blue, the tool keeps its colour.
    let before = h.state().comment_prefs.style(CommentTool::StrikeOut);
    open_comment_props(&mut h, 0, 0);
    h.run_steps(2);
    set_prop(&mut h, PROP_COLOUR, FieldValue::Color(blue.map(|c| c as f32)));
    accept_props(&mut h);
    assert_eq!(h.state().comment_prefs.style(CommentTool::StrikeOut).color, before.color);

    // With it: green, and a 0% opacity is clamped so the next strikethrough stays visible.
    open_comment_props(&mut h, 0, 0);
    h.run_steps(2);
    set_prop(&mut h, PROP_COLOUR, FieldValue::Color(green.map(|c| c as f32)));
    set_prop(&mut h, "opacity", FieldValue::Number(0.0));
    set_prop(&mut h, "make_default", FieldValue::Bool(true));
    accept_props(&mut h);
    let st = h.state().comment_prefs.style(CommentTool::StrikeOut);
    assert_eq!((st.color, st.opacity), (green, 0.1));
    let fresh = {
        let mut fresh = crate::control::new_app();
        fresh.apply_settings(&h.state().settings_snapshot());
        fresh.comment_prefs.style(CommentTool::StrikeOut).color
    };
    assert_eq!(fresh, green, "the default persisted");
    drag_pt(&mut h, (18.0, 155.0), (150.0, 155.0));
    let c = comments(&h);
    let made = c.iter().rfind(|a| a.subtype == "StrikeOut").and_then(|a| a.color).expect("a colougreen strikethrough");
    assert!(made.iter().zip(green).all(|(m, r)| (f64::from(*m) - r).abs() < 0.01), "the next strikethrough is green: {made:?}");
}

/// A text box with neither a border nor a fill is all text: Make Properties Default (and the
/// context menu's Make Current Properties Default) gives the tool the opacity its text is drawn
/// at, the one Comment Properties shows, not the comment's /CA.
#[test]
fn a_text_only_text_box_made_default_gives_the_tool_its_texts_opacity() {
    use pdfcraft_engine::{Edit, NewAnnotation, Shape, Style};
    use pdfcraft_ui_egui::comments::CommentTool;
    let mut h = harness(|_| {});
    let shape = Shape::TextBox { rect: [100.0, 600.0, 220.0, 630.0], font_size: 12.0 };
    let style = Style { width: 0.0, fill: None, ..Style::default_for(&shape) };
    let ctx_h = h.ctx.clone();
    assert!(
        h.state_mut()
            .apply_edit(&ctx_h, Edit::AddAnnotation(NewAnnotation { page: 0, shape, style, contents: "Faint".into(), author: "Tester".into() }))
    );
    h.run_steps(2);
    let index = comments(&h).iter().find(|a| a.contents.as_deref() == Some("Faint")).map(|a| a.index).expect("the text box");
    // As the automation's comment_edit sets it.
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().apply_edit(&ctx_h, Edit::PartOpacity { page: 0, index, fill: None, text: Some(0.5) }));
    h.run_steps(2);
    let opacity = |h: &Harness<'static, PdfCraftApp>| h.state().comment_prefs.style(CommentTool::TextBox).opacity;
    assert!((opacity(&h) - 1.0).abs() < 1e-6);
    open_comment_props(&mut h, 0, index);
    h.run_steps(2);
    set_prop(&mut h, "make_default", FieldValue::Bool(true));
    accept_props(&mut h);
    assert!((opacity(&h) - 0.5).abs() < 1e-6, "the text's 50 %: {}", opacity(&h));
    // The context menu's way, from a tool at 100 % again.
    h.state_mut().comment_prefs.set_opacity(CommentTool::TextBox, 1.0);
    h.state_mut().make_comment_default(0, index);
    assert!((opacity(&h) - 0.5).abs() < 1e-6, "the text's 50 %: {}", opacity(&h));
}

/// Make Properties Default on a text box with a border colour of its own gives the Text box tool
/// that colour.
#[test]
fn a_text_box_made_default_gives_the_tool_its_border_colour() {
    use pdfcraft_engine::{Edit, NewAnnotation, Shape, Style};
    use pdfcraft_ui_egui::comments::CommentTool;
    let mut h = harness(|_| {});
    let shape = Shape::TextBox { rect: [100.0, 600.0, 220.0, 630.0], font_size: 12.0 };
    let style = Style { width: 1.0, ..Style::default_for(&shape) };
    let ctx_h = h.ctx.clone();
    assert!(
        h.state_mut()
            .apply_edit(&ctx_h, Edit::AddAnnotation(NewAnnotation { page: 0, shape, style, contents: "Framed".into(), author: "Tester".into() }))
    );
    h.run_steps(2);
    let index = comments(&h).iter().find(|a| a.contents.as_deref() == Some("Framed")).map(|a| a.index).expect("the text box");
    let red = Some([0.89, 0.13, 0.13]);
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().apply_edit(&ctx_h, Edit::BorderColorAnnotation { page: 0, index, color: red }));
    assert_ne!(h.state().comment_prefs.style(CommentTool::TextBox).border_color, red);
    open_comment_props(&mut h, 0, index);
    h.run_steps(2);
    set_prop(&mut h, "make_default", FieldValue::Bool(true));
    accept_props(&mut h);
    assert_eq!(h.state().comment_prefs.style(CommentTool::TextBox).border_color, red);
}

/// The Text box tool's Border colour button shows the selected text box's own border colour
/// (Comment Properties may have given it one), and the tool's again with none selected.
#[test]
fn the_border_colour_button_shows_the_selected_text_boxs_colour() {
    use pdfcraft_engine::{Edit, NewAnnotation, Shape, Style};
    let mut h = harness(|app| app.set_option(&egui::Context::default(), "quick", "freetext").unwrap());
    let shape = Shape::TextBox { rect: [100.0, 600.0, 220.0, 630.0], font_size: 12.0 };
    let style = Style { width: 1.0, ..Style::default_for(&shape) };
    let ctx_h = h.ctx.clone();
    assert!(
        h.state_mut()
            .apply_edit(&ctx_h, Edit::AddAnnotation(NewAnnotation { page: 0, shape, style, contents: "Framed".into(), author: "Tester".into() }))
    );
    h.run_steps(2);
    let index = comments(&h).iter().find(|a| a.contents.as_deref() == Some("Framed")).map(|a| a.index).expect("the text box");
    // A border colour of its own, which the tool doesn't have.
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().apply_edit(&ctx_h, Edit::BorderColorAnnotation { page: 0, index, color: Some([0.0, 0.47, 0.84]) }));
    h.run_steps(2);
    // The colour the well shows: the swatch's colour value in the access tree, which is the
    // colour it is drawn in.
    let shown = |h: &Harness<'static, PdfCraftApp>| NodeT::accesskit_node(&h.get_by_label("Border colour")).data().color_value();
    // A new text box comes selected.
    h.state_mut().views[0].session.comments.clear_selection();
    h.run_steps(3);
    let tool = shown(&h).expect("the swatch's colour value");
    h.state_mut().views[0].session.comments.select_only((0, index));
    h.run_steps(3);
    let own = shown(&h).expect("the swatch's colour value");
    assert_ne!(own, tool, "the selected text box's own colour, not the tool's");
    h.state_mut().views[0].session.comments.clear_selection();
    h.run_steps(3);
    assert_eq!(shown(&h), Some(tool), "the tool's colour again");
    // The well shows the text box's blue: the tool given the same colour shows the same value.
    h.state_mut().comment_prefs.set_border_color(pdfcraft_ui_egui::comments::CommentTool::TextBox, Some([0.0, 0.47, 0.84]));
    h.run_steps(3);
    assert_eq!(shown(&h), Some(own), "the selected text box's colour is the blue it was given");
}

/// #1302: making an arrow's properties the default updates the Arrow tool, not later plain lines.
#[test]
fn an_arrows_default_does_not_restyle_lines() {
    use pdfcraft_ui_egui::comments::CommentTool;

    let mut h = harness(|app| {
        app.set_option(&egui::Context::default(), "panel", "comments").unwrap();
        app.set_option(&egui::Context::default(), "quick", "arrow").unwrap();
    });
    drag_pt(&mut h, (20.0, 40.0), (140.0, 80.0));
    h.set_option("quick", "select").unwrap();
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(
        &ctx_h,
        pdfcraft_engine::Edit::StyleAnnotation {
            page: 0,
            index: 0,
            color: Some([0.0, 0.47, 0.84]),
            opacity: Some(0.5),
            width: Some(6.0),
            endings: None,
        },
    );
    h.run_steps(2);
    h.get_by_label("More").click();
    h.run_steps(2);
    h.get_by_label("Make Current Properties Default").click();
    h.run_steps(3);
    let arrow = h.state().comment_prefs.style(CommentTool::Arrow);
    let line = h.state().comment_prefs.style(CommentTool::Line);
    assert_eq!((arrow.color, arrow.opacity, arrow.width), ([0.0, 0.47, 0.84], 0.5, 6.0));
    assert_eq!((line.color, line.opacity, line.width), ([0.89, 0.13, 0.13], 1.0, 2.0), "plain lines keep their own default");

    h.set_option("quick", "arrow").unwrap();
    drag_pt(&mut h, (20.0, 100.0), (140.0, 140.0));
    h.set_option("quick", "line").unwrap();
    drag_pt(&mut h, (160.0, 40.0), (260.0, 80.0));
    let c = comments(&h);
    let arrows = c.iter().filter(|a| a.subtype == "Line").collect::<Vec<_>>();
    assert!(arrows.len() >= 3, "{c:?}");
    let next_arrow = &arrows[1];
    let plain = arrows.last().unwrap();
    assert_eq!(next_arrow.color, Some([0.0, 0.47, 0.84]));
    assert_eq!(plain.color, Some([0.89, 0.13, 0.13]));
}

#[test]
fn highlighting_an_area_off_the_text() {
    let mut h = harness(|app| app.set_option(&egui::Context::default(), "quick", "highlight").unwrap());
    // Blank space below the text line (text is at y 150).
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    let c = comments(&h);
    assert_eq!(c.len(), 1, "{c:?}");
    assert_eq!((c[0].subtype.as_str(), c[0].quads.len()), ("Highlight", 1));
    let q = c[0].quads[0];
    assert!((q[0] - 40.0).abs() < 3.0 && (q[1] - 100.0).abs() < 3.0, "{q:?}");
    // Dragging over text still highlights the text (once the text layer is in).
    support::wait_until(&mut h, "the page's text layer", |app| {
        let v = &app.views[0];
        v.glyph_screen_pos(0, 4).is_some() && v.glyph_screen_pos(0, 14).is_some()
    });
    let (a, b) = {
        let v = &h.state().views[0];
        (v.glyph_screen_pos(0, 4).expect("text layer"), v.glyph_screen_pos(0, 14).expect("glyph"))
    };
    drag(&mut h, a, b);
    assert_eq!(comments(&h).len(), 2);
}

/// A Replace Text proposal is picked by its strikeout too, not only by its small caret: the
/// strikeout is grouped under the caret (`/RT /Group`), and a click on it selects the caret, the
/// proposal, so it can be deleted or restyled from either part.
#[test]
fn clicking_a_replace_text_strikeout_selects_the_proposal() {
    let pdf = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Annots [5 0 R 4 0 R] >> endobj
4 0 obj << /Type /Annot /Subtype /Caret /Rect [135 95 145 105] /C [0 0 1] /T (Bob) /IT /Replace /Contents (slow red) >> endobj
5 0 obj << /Type /Annot /Subtype /StrikeOut /Rect [40 100 130 120] /QuadPoints [40 120 130 120 40 100 130 100] /C [1 0 0] /T (Bob) /IT /StrikeOutTextEdit /IRT 4 0 R /RT /Group >> endobj
trailer << /Root 1 0 R >>
%%EOF";
    let mut h = harness(move |app| {
        app.open_bytes(&egui::Context::default(), "replace.pdf", None, pdf.to_vec()).expect("opens");
        app.close_tab(0);
    });
    h.run_steps(6);
    let caret = comments(&h).into_iter().find(|a| a.subtype == "Caret").expect("the caret");
    let p = at(&h, 70.0, 110.0);
    click(&mut h, p);
    assert_eq!(h.state().views[0].session.comments.selection(), vec![(caret.page, caret.index)], "the strikeout picks the proposal");
    // Review on #5689 (CodeRabbit): a drag from the strikeout, or from the caret, moves neither:
    // the proposal lies on the text it replaces, and the caret alone would leave the strikeout.
    let caret_at = at(&h, 140.0, 100.0);
    for from in [p, caret_at] {
        drag(&mut h, from, from + egui::vec2(60.0, 40.0));
        h.run_steps(3);
        let now = comments(&h).into_iter().find(|a| a.subtype == "Caret").expect("the caret");
        assert_eq!(now.rect, caret.rect, "the caret stays on the text");
    }
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), None, "nothing was moved");
}

#[test]
fn replacing_text_strikes_it_and_adds_a_caret() {
    let mut h = harness(|app| app.set_option(&egui::Context::default(), "quick", "replace").unwrap());
    // Review on #5689 (CodeRabbit): both parts take the Replace text tool's style.
    h.state_mut().comment_prefs.set_color(pdfcraft_ui_egui::comments::CommentTool::ReplaceText, [0.1, 0.6, 0.2]);
    drag_pt(&mut h, (20.0, 155.0), (120.0, 155.0));
    assert!(h.state().views[0].session.comments.composer.is_some(), "the composer asks for the replacement");
    field(&h, "Replacement text").type_text("slow red");
    h.run_steps(2);
    h.get_by_label("Post").click();
    h.run_steps(3);
    let c = comments(&h);
    let strike = c.iter().find(|a| a.subtype == "StrikeOut").expect("a strikeout");
    let caret = c.iter().find(|a| a.subtype == "Caret").expect("a caret");
    assert_eq!(caret.contents.as_deref(), Some("slow red"));
    assert_eq!((strike.color, caret.color), (Some([0.1, 0.6, 0.2]), Some([0.1, 0.6, 0.2])), "one style for the proposal");
    // The caret is the group's primary, the strikeout its `/RT /Group` subordinate (ISO 32000-2 §12.5.6.2).
    assert_eq!(strike.in_reply_to, caret.name, "the strikeout is grouped with the caret");
    assert!(strike.is_group_member() && !caret.grouped && !caret.is_reply());
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Replace text"));
    assert_eq!(h.state().quick_tool, QuickTool::Select);
    // Both members are the Replace text tool's, not Strikethrough's or Insert text's (#3947), so
    // Make Current Properties Default updates the Replace text tool.
    assert_eq!(pdfcraft_ui_egui::comments::tool_for(strike), Some(pdfcraft_ui_egui::comments::CommentTool::ReplaceText));
    assert_eq!(pdfcraft_ui_egui::comments::tool_for(caret), Some(pdfcraft_ui_egui::comments::CommentTool::ReplaceText));
    // The panel's card is the caret's, the group's primary: it shows the replacement, and
    // nothing is named Strikethrough.
    h.set_option("panel", "comments").unwrap();
    h.run_steps(3);
    // One card, the caret's, showing the replacement text; sorted by type, its group is named
    // Replace text (not Insert text, and no Strikethrough).
    assert_eq!(h.query_all_by_label_contains("slow red").count(), 1, "one comment with the replacement text");
    h.state_mut().views[0].session.comments.panel.sort = CommentSort::Type;
    h.run_steps(3);
    assert!(h.query_by_label("Replace text, 1").is_some(), "the panel names the comment Replace text");
    assert!(h.query_by_label_contains("Insert text").is_none(), "not Insert text");
    assert!(h.query_by_label_contains("Strikethrough").is_none(), "not Strikethrough");
}

#[test]
fn attaching_a_file_as_a_comment() {
    // The picker answers with a file in a folder of its own, as a user's choice would.
    let dir = std::env::temp_dir().join(format!("pdfcraft-attach-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("notes.txt");
    std::fs::write(&file, b"remember the milk").unwrap();
    let files = std::sync::Arc::new(ScriptedFiles::answering([Picked::Paths(vec![file.clone()])]));
    let asked = files.clone();
    let mut h = harness_on_files(files, |app| app.set_option(&egui::Context::default(), "quick", "attach").unwrap());
    click_pt(&mut h, (250.0, 180.0));
    h.run_steps(3);
    assert_eq!(asked.requests().len(), 1, "the tool asks the picker once");
    std::fs::remove_dir_all(&dir).unwrap();
    let c = comments(&h);
    assert_eq!((c.len(), c[0].subtype.as_str(), c[0].contents.as_deref()), (1, "FileAttachment", Some("notes.txt")));
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert!(doc.info.attachments.iter().any(|a| a.name == "notes.txt"), "listed in the Attachments panel");
    assert_eq!(s.quick_tool, QuickTool::Select);
}

/// One 300×200 page with a paperclip FileAttachment comment at (100, 100)–(120, 124) carrying
/// `inner.pdf`, and no /Contents: the card shows only "Attachment" (#2565). The cross-reference
/// table keeps the reader from mistaking the embedded PDF's objects for the outer ones.
fn attachment_comment_fixture() -> Vec<u8> {
    let inner = "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
                 3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Annots [4 0 R] >>".into(),
        "<< /Type /Annot /Subtype /FileAttachment /Rect [100 100 120 124] /Name /Paperclip /T (Reviewer) /M (D:20261014105200) /FS 5 0 R >>".into(),
        "<< /Type /Filespec /F (inner.pdf) /UF (inner.pdf) /EF << /F 6 0 R >> >>".into(),
        format!("<< /Type /EmbeddedFile /Length {} >>\nstream\n{inner}\nendstream", inner.len()),
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

fn attachment_harness(setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "annexed.pdf", None, attachment_comment_fixture()).expect("opens");
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            setup(&mut app);
            app
        }
    });
    h.run_steps(4);
    crate::support::settle(&mut h);
    h
}

fn open_tab_names(h: &Harness<'static, PdfCraftApp>) -> Vec<String> {
    let s = h.state();
    s.views.iter().filter_map(|v| s.session.get(v.id)).map(|d| d.name.clone()).collect()
}

/// Row of `comment_menu_rows_follow_permissions_and_run_their_action`: the page's menu on an
/// attachment comment opens its file in a new tab.
fn attachment_page_menu_row() {
    let mut h = attachment_harness(|app| {
        app.set_option(&egui::Context::default(), "panel", "comments").unwrap();
        app.set_option(&egui::Context::default(), "quick", "select").unwrap();
    });
    assert_eq!(comments(&h)[0].subtype, "FileAttachment");
    click_pt(&mut h, (110.0, 112.0));
    assert_eq!(h.state().views[0].session.comments.selection(), vec![(0, 0)], "the attachment comment is selected");
    right_click_pt(&mut h, (110.0, 112.0));
    h.get_by_label("Copy text");
    // The Attachments panel is closed, so these items can only come from the comment's menu.
    h.get_by_label("Save attachment…");
    h.get_by_label("Open in a new tab").click();
    h.run_steps(4);
    assert_eq!(open_tab_names(&h), ["annexed.pdf", "inner.pdf"]);
}

/// Row of `panel_and_card_actions_become_engine_edits`: a card's "…" menu on an attachment
/// comment opens its file in a new tab.
fn attachment_card_menu_row() {
    let mut h = attachment_harness(|app| app.set_option(&egui::Context::default(), "panel", "comments").unwrap());
    h.state_mut().views[0].session.comments.select_only((0, 0));
    h.run_steps(2);
    h.get_by_label("More").click();
    h.run_steps(2);
    h.get_by_label("Copy text");
    h.get_by_label("Save attachment…");
    h.get_by_label("Open in a new tab").click();
    h.run_steps(4);
    assert_eq!(open_tab_names(&h), ["annexed.pdf", "inner.pdf"]);
}

#[test]
fn erasing_part_of_a_drawing() {
    let mut h = harness(|app| app.set_option(&egui::Context::default(), "quick", "ink").unwrap());
    drag_pt(&mut h, (40.0, 60.0), (260.0, 60.0));
    assert_eq!(comments(&h).len(), 1);
    h.set_option("quick", "eraser").unwrap();
    drag_pt(&mut h, (150.0, 90.0), (150.0, 30.0));
    let c = comments(&h);
    assert_eq!(c.len(), 1);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(doc.can_undo(), Some("Erase"));
    // Rubbing along the rest removes the drawing.
    drag_pt(&mut h, (40.0, 60.0), (260.0, 60.0));
    assert!(comments(&h).is_empty(), "{:?}", comments(&h));
}

#[test]
fn the_eraser_rubs_as_wide_as_its_thickness_setting() {
    // #3747: the eraser's size changed nothing; it always rubbed 6 pixels either side.
    let stroke_near = |width: f64| {
        let mut h = harness(|app| app.set_option(&egui::Context::default(), "quick", "ink").unwrap());
        drag_pt(&mut h, (40.0, 60.0), (260.0, 60.0));
        assert_eq!(comments(&h).len(), 1);
        let zoom = h.state().views[0].page_screen_rect(0).expect("page 1 on screen").width() / 300.0;
        let app = h.state_mut();
        app.comment_prefs.set_width(pdfcraft_ui_egui::comments::CommentTool::Eraser, width);
        app.set_option(&egui::Context::default(), "quick", "eraser").unwrap();
        // Along the whole drawing, 20 pixels above it: out of reach of the default eraser
        // (6 pixels), within reach of the largest (36).
        let y = 60.0 + 20.0 / zoom;
        drag_pt(&mut h, (30.0, y), (270.0, y));
        comments(&h).len()
    };
    assert_eq!(stroke_near(2.0), 1, "the default eraser passes by");
    assert_eq!(stroke_near(12.0), 0, "the largest rubs the drawing out");
}

#[test]
fn hovering_a_comment_shows_its_author_and_text() {
    // A square comment by Ada over the first page's text.
    let pdf = String::from_utf8_lossy(TEXT_FIXTURE).replace("/Contents 5 0 R /Resources", "/Contents 5 0 R /Annots [8 0 R] /Resources").replace(
        "trailer",
        "8 0 obj << /Type /Annot /Subtype /Square /Rect [50 50 150 120] /C [1 0 0] /T (Ada) /Contents (Check this figure) >> endobj\ntrailer",
    );
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "hover.pdf", None, pdf.into_bytes()).expect("opens");
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            // No Comments panel: the comment's text should only appear in the hover popup.
            app.set_option(&egui::Context::default(), "panel", "none").unwrap();
            app
        }
    });
    h.run_steps(4);
    crate::support::settle(&mut h);
    assert!(h.query_by_label_contains("Check this figure").is_none(), "no popup before hovering");
    let p = at(&h, 100.0, 85.0);
    h.hover_at(p);
    h.run_steps(3);
    h.get_by_label_contains("Check this figure");
    h.get_by_label_contains("Ada");
    // The tip is drawn above every window. A dialog must hide it, or the author floats over the dialog.
    h.set_option("dialog", "shortcuts").unwrap();
    h.hover_at(p);
    h.run_steps(3);
    assert!(h.query_by_label_contains("Check this figure").is_none(), "a dialog covers the page hover");
}

#[test]
fn preference_hides_author_only_highlight_hover_previews() {
    // A highlight annotation with author Ada and empty contents over the first page.
    let pdf = String::from_utf8_lossy(TEXT_FIXTURE).replace("/Contents 5 0 R /Resources", "/Contents 5 0 R /Annots [8 0 R] /Resources").replace(
        "trailer",
        "8 0 obj << /Type /Annot /Subtype /Highlight /Rect [50 50 150 120] /C [1 1 0] /T (Ada) /QuadPoints [50 120 150 120 50 50 150 50] >> endobj\ntrailer",
    );
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "highlight_hover.pdf", None, pdf.into_bytes()).expect("opens");
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "panel", "none").unwrap();
            app
        }
    });
    h.run_steps(4);
    crate::support::settle(&mut h);

    let p = at(&h, 100.0, 85.0);
    // By default, author-only hover is shown.
    h.hover_at(p);
    h.run_steps(3);
    h.get_by_label("Ada");

    // Enabling preference hides author-only highlight hover.
    h.set_option("hide-empty-highlight-hover", "true").unwrap();
    h.hover_at(p);
    h.run_steps(3);
    assert!(h.query_by_label("Ada").is_none(), "author-only hover must be suppressed");
}

/// Contributor-original synthetic markup, with the first highlight ending on the next line.
fn markup_hover_fixture(subtype: &str, rotation: u16, extra: &str, refs: &str) -> Vec<u8> {
    let content = "BT /F1 14 Tf 20 150 Td (Before the marked words) Tj 0 -40 Td (Marked words then other words) Tj ET\n";
    String::from_utf8_lossy(TEXT_FIXTURE)
        .replace("/Contents 5 0 R /Resources", &format!("/Contents 5 0 R /Rotate {rotation} /Annots [8 0 R {refs}] /Resources"))
        .replace("/Length 55", &format!("/Length {}", content.len()))
        .replace("BT /F1 14 Tf 20 150 Td (The quick brown fox) Tj ET\n", content)
        .replace(
            "trailer",
            &format!(
                "8 0 obj << /Type /Annot /Subtype /{subtype} /Rect [20 105 250 165] \
                 /QuadPoints [120 165 250 165 120 145 250 145 20 125 100 125 20 105 100 105] \
                 /C [1 1 0] /T (Ada) /Contents (First multiline comment) >> endobj\n{extra}\ntrailer"
            ),
        )
        .into_bytes()
}

fn markup_hover_harness(pdf: Vec<u8>, view_rotation: u16) -> Harness<'static, PdfCraftApp> {
    // Give the fixture a complete xref so byte comparisons do not measure repair-on-save.
    let doc = pdfcraft_cos::Document::open(std::sync::Arc::new(pdf)).expect("synthetic fixture parses");
    let pdf = pdfcraft_cos::write_full(&doc, &Default::default()).expect("writes complete fixture");
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "markup.pdf", None, pdf).expect("opens synthetic markup");
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "panel", "none").unwrap();
            // Small enough to show the whole page in every rotation: a point hovered below the window
            // is not over the page.
            app.set_option(&egui::Context::default(), "zoom", "100").unwrap();
            app.views[0].view.rotation = view_rotation;
            app
        }
    });
    h.run_steps(4);
    crate::support::settle(&mut h);
    h
}

/// Includes the document's crop/rotation and the independent Rotate View transform.
fn hover_markup_at(h: &mut Harness<'static, PdfCraftApp>, x: f32, y: f32) {
    let p = {
        let app = h.state();
        let v = &app.views[0];
        let info = &app.session.get(v.id).unwrap().info;
        let page = &info.pages[0];
        let xf = pdfcraft_ui_egui::canvas::PageXform {
            rect: v.page_screen_rect(0).expect("page on screen"),
            rot: v.view.rotation,
            pw: page.width,
            ph: page.height,
        };
        let p = xf.user_rect(info, 0, [x, y, x, y]).center();
        assert!(v.viewport_rect().contains(p), "user point ({x}, {y}) is on screen, at {p:?}");
        p
    };
    h.hover_at(p);
    h.run_steps(3);
}

#[test]
fn imported_commented_markup_keeps_author_at_rotated_zoomed_page_edges() {
    let _gpu = std::env::var_os("PDFCRAFT_SHOTS").map(|_| crate::gpu());
    for subtype in ["Highlight", "Underline", "StrikeOut", "Squiggly"] {
        for (page_rotation, view_rotation) in [(0, 0), (90, 0), (0, 90), (90, 90)] {
            for zoom in ["50", "125"] {
                let pdf = String::from_utf8(markup_hover_fixture(subtype, page_rotation, "", ""))
                    .unwrap()
                    .replace("/Rect [20 105 250 165]", "/Rect [270 180 300 200]")
                    .replace(
                        "/QuadPoints [120 165 250 165 120 145 250 145 20 125 100 125 20 105 100 105]",
                        "/QuadPoints [270 200 300 200 270 180 300 180]",
                    );
                let mut h = markup_hover_harness(pdf.into_bytes(), view_rotation);
                h.set_option("zoom", zoom).unwrap();
                h.run_steps(4);
                support::settle(&mut h);
                hover_markup_at(&mut h, 285.0, 190.0);
                h.get_by_label("Ada\nFirst multiline comment");
                if subtype == "Highlight"
                    && matches!((page_rotation, view_rotation, zoom), (0, 0, "50") | (90, 90, "125"))
                    && let Ok(dir) = std::env::var("PDFCRAFT_SHOTS")
                {
                    h.render().unwrap().save(format!("{dir}/edge-comment-{page_rotation}-{view_rotation}-{zoom}.png")).unwrap();
                }
            }
        }
    }
}

#[test]
fn multiline_markup_hover_stays_on_marked_lines() {
    // #3541: /Rect encloses both lines, including text before/after the selection and the gap.
    for subtype in ["Highlight", "Underline", "StrikeOut", "Squiggly"] {
        for (page_rotation, view_rotation) in [(0, 0), (90, 0), (180, 0), (270, 0), (0, 90), (0, 180), (0, 270), (90, 270)] {
            let pdf = markup_hover_fixture(subtype, page_rotation, "", "");
            let mut h = markup_hover_harness(pdf, view_rotation);
            let original = h.state().session.get(h.state().views[0].id).unwrap().saved_bytes();
            for (x, y, marked) in
                [(160.0, 155.0, true), (60.0, 115.0, true), (60.0, 155.0, false), (180.0, 115.0, false), (60.0, 135.0, false), (60.0, 75.0, false)]
            {
                hover_markup_at(&mut h, x, y);
                assert_eq!(
                    h.query_by_label_contains("First multiline comment").is_some(),
                    marked,
                    "{subtype}, page/view rotation {page_rotation}/{view_rotation}, user point ({x}, {y})"
                );
            }
            let app = h.state();
            let id = app.views[0].id;
            let doc = app.session.get(id).unwrap();
            assert!(!doc.dirty, "hovering never edits the document");
            assert_eq!(doc.can_undo(), None);
            assert_eq!(app.session.save_bytes(id).unwrap().as_slice(), original.as_slice());
        }
    }
}

#[test]
fn overlapping_markup_hover_shows_the_topmost_comment() {
    // A later annotation can also cover a genuinely marked part of the older highlight.
    let other = "9 0 obj << /Type /Annot /Subtype /Highlight /Rect [150 145 200 165] \
                 /QuadPoints [150 165 200 165 150 145 200 145] /Contents (Topmost comment) >> endobj";
    let mut h = markup_hover_harness(markup_hover_fixture("Highlight", 0, other, "9 0 R"), 0);
    hover_markup_at(&mut h, 170.0, 155.0);
    h.get_by_label_contains("Topmost comment");
    assert!(h.query_by_label_contains("First multiline comment").is_none());
    hover_markup_at(&mut h, 230.0, 155.0);
    h.get_by_label_contains("First multiline comment");
    assert!(h.query_by_label_contains("Topmost comment").is_none());
}

#[test]
fn overlapping_markup_hover_uses_annotation_index_with_different_top_edges() {
    // Inspection sorts by position, while /Annots determines paint order. The later
    // highlight wins the overlap whether its upper edge is above or below the first's.
    for top in [175, 160] {
        let other = format!(
            "9 0 obj << /Type /Annot /Subtype /Highlight /Rect [150 145 200 {top}] \
             /QuadPoints [150 {top} 200 {top} 150 145 200 145] /Contents (Topmost comment) >> endobj"
        );
        let mut h = markup_hover_harness(markup_hover_fixture("Highlight", 0, &other, "9 0 R"), 0);
        hover_markup_at(&mut h, 170.0, 155.0);
        h.get_by_label_contains("Topmost comment");
        assert!(h.query_by_label_contains("First multiline comment").is_none(), "later annotation with top {top} owns the overlap");
        hover_markup_at(&mut h, 230.0, 155.0);
        h.get_by_label_contains("First multiline comment");
        assert!(h.query_by_label_contains("Topmost comment").is_none());
    }
}

#[test]
fn markup_hover_skips_popup_containers() {
    // /Popup holds the parent's comment window; it is not a second comment in paint order.
    let popup = "9 0 obj << /Type /Annot /Subtype /Popup /Parent 8 0 R /Rect [150 145 200 165] >> endobj";
    let mut h = markup_hover_harness(markup_hover_fixture("Highlight", 0, popup, "9 0 R"), 0);
    hover_markup_at(&mut h, 170.0, 155.0);
    h.get_by_label_contains("First multiline comment");
    assert!(h.query_by_label("Popup").is_none());
}

#[test]
fn markup_hover_without_quads_keeps_the_rectangle_fallback() {
    for quads in ["", "/QuadPoints [20 105 250 165]"] {
        let pdf = markup_hover_fixture("Highlight", 0, "", "");
        let pdf = String::from_utf8(pdf)
            .unwrap()
            .replace("/QuadPoints [120 165 250 165 120 145 250 145 20 125 100 125 20 105 100 105]", quads)
            .into_bytes();
        let mut h = markup_hover_harness(pdf, 0);
        assert!(comments(&h)[0].quads.is_empty());
        hover_markup_at(&mut h, 180.0, 115.0);
        h.get_by_label_contains("First multiline comment");
        hover_markup_at(&mut h, 270.0, 115.0);
        assert!(h.query_by_label_contains("First multiline comment").is_none());
    }
}

#[test]
fn the_opacity_set_for_a_tool_goes_into_its_new_comments() {
    use pdfcraft_ui_egui::comments::CommentTool;
    let mut h = harness(|app| app.comment_prefs.set_opacity(CommentTool::StrikeOut, 0.5));
    // Clamped: an invisible comment can't be found again.
    h.state_mut().comment_prefs.set_opacity(CommentTool::Underline, 0.0);
    assert!((h.state().comment_prefs.style(CommentTool::Underline).opacity - 0.1).abs() < 1e-9);
    h.state_mut().views[0].select_text(0, 4, 8);
    assert!(h.execute("comment.strikeout"));
    h.run_steps(2);
    let c = comments(&h);
    assert_eq!(c.len(), 1, "{c:?}");
    // The new comment carries /CA 0.5 (read back from the document's bytes).
    let bytes = h.state().session.get(h.state().views[0].id).unwrap().bytes.as_ref().clone();
    let doc = pdfcraft_cos::Document::open(std::sync::Arc::new(bytes)).unwrap();
    let opacities: Vec<f64> = doc
        .object_numbers()
        .into_iter()
        .filter_map(|n| doc.try_get(n).ok())
        .filter_map(|o| o.as_dict().cloned())
        .filter(|d| d.name(b"Subtype") == Some(b"StrikeOut"))
        .filter_map(|d| d.get(b"CA").and_then(|v| v.as_f64()))
        .collect();
    assert_eq!(opacities, [0.5]);
    // The quick bar offers the control.
    h.set_option("quick", "strikeout").unwrap();
    h.run_steps(2);
    h.get_by_label("Opacity");
}

/// #225: comment tools open the Comments panel until the user closes it; then it stays closed,
/// across restarts, until they open it again.
#[test]
fn a_closed_comments_panel_stays_closed_when_picking_comment_tools() {
    use pdfcraft_ui_egui::RightPanel;
    let mut h = harness(|_| {});
    assert_eq!(crate::ui::right(h.state()), None);
    assert!(h.execute("comment.highlight"));
    h.run_steps(2);
    assert_eq!(crate::ui::right(h.state()), Some(RightPanel::Comments), "Acrobat opens Comments with the tools");
    // In IconicMode::Rail clicking the active Comments rail button toggles the panel closed.
    h.get_by_role_and_label(Role::Button, "Comments").click();
    h.run_steps(2);
    assert_eq!(crate::ui::right(h.state()), None);
    for tool in ["select", "comment.underline", "comment.note", "comment.highlight"] {
        if tool == "select" {
            h.state_mut().quick_tool = QuickTool::Select;
        } else {
            assert!(h.execute(tool));
        }
        h.run_steps(2);
        assert_eq!(crate::ui::right(h.state()), None, "{tool} reopened the closed Comments panel");
    }
    let mut again = crate::control::new_app();
    again.apply_settings(&h.state().settings_snapshot());
    assert!(again.comments_panel_closed, "the choice survives a restart");
    // Opening it again from the rail lets the tools open it once more.
    h.get_by_role_and_label(Role::Button, "Comments").click();
    h.run_steps(2);
    assert_eq!(crate::ui::right(h.state()), Some(RightPanel::Comments));
    assert!(!h.state().comments_panel_closed);
    // What the rail and View menu call; "Bookmarks" labels two controls on screen.
    h.state_mut().choose_right_panel(Some(RightPanel::Bookmarks));
    h.state_mut().choose_right_panel(None);
    h.run_steps(2);
    assert!(!h.state().comments_panel_closed, "switching to and closing another panel isn't closing Comments");
    assert!(h.execute("comment.strikeout"));
    h.run_steps(2);
    assert_eq!(crate::ui::right(h.state()), Some(RightPanel::Comments));
}

#[test]
fn text_box_and_callout_properties_include_fill_and_thickness() {
    use pdfcraft_engine::{Edit, LineEnding, NewAnnotation, Shape};
    use pdfcraft_ui_egui::comments::CommentTool;
    let mut h = harness(|_| {});
    for (tool, shape) in [
        (CommentTool::TextBox, Shape::TextBox { rect: [40.0, 600.0, 240.0, 640.0], font_size: 12.0 }),
        (
            CommentTool::Callout,
            Shape::Callout {
                rect: [40.0, 500.0, 240.0, 540.0],
                knee: [30.0, 460.0],
                point: [10.0, 440.0],
                font_size: 12.0,
                ending: LineEnding::OpenArrow,
            },
        ),
    ] {
        let index = comments(&h).len();
        let ctx_h = h.ctx.clone();
        h.state_mut().apply_edit(
            &ctx_h,
            Edit::AddAnnotation(NewAnnotation { page: 0, shape, style: Default::default(), contents: "Hello".into(), author: "Ada".into() }),
        );
        h.run_steps(2);
        open_comment_props(&mut h, 0, index);
        h.run_steps(2);
        h.get_by_label("Fill colour");
        {
            let doc = h.state().session.get(h.state().views[0].id).unwrap();
            let p = doc.comment_props(0, index).unwrap();
            assert!(p.fillable && p.width.is_some());
        }
        set_prop(&mut h, PROP_FILL, FieldValue::Color([1.0; 3]));
        set_prop(&mut h, "width", FieldValue::Number(3.0));
        set_prop(&mut h, "make_default", FieldValue::Bool(true));
        accept_props(&mut h);
        let doc = h.state().session.get(h.state().views[0].id).unwrap();
        let p = doc.comment_props(0, index).unwrap();
        assert_eq!((p.fill, p.width), (Some([1.0; 3]), Some(3.0)));
        assert_eq!(h.state().comment_prefs.style(tool).fill, Some([1.0; 3]));
        assert_eq!(h.state().comment_prefs.style(tool).width, 3.0);
    }
}

#[test]
fn text_comments_can_save_a_zero_width_default() {
    use pdfcraft_engine::{Edit, NewAnnotation, Shape};
    use pdfcraft_ui_egui::comments::CommentTool;
    let mut h = harness(|_| {});
    let shape = Shape::TextBox { rect: [40.0, 600.0, 240.0, 640.0], font_size: 12.0 };
    let mut style = pdfcraft_engine::Style::default_for(&shape);
    style.width = 0.0;
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(&ctx_h, Edit::AddAnnotation(NewAnnotation { page: 0, shape, style, contents: "Hello".into(), author: "Ada".into() }));
    h.run_steps(2);
    open_comment_props(&mut h, 0, 0);
    h.run_steps(2);
    {
        let s = h.state();
        assert_eq!(s.session.get(s.views[0].id).unwrap().comment_props(0, 0).unwrap().width, Some(0.0));
    }
    set_prop(&mut h, "make_default", FieldValue::Bool(true));
    accept_props(&mut h);
    assert_eq!(h.state().comment_prefs.style(CommentTool::TextBox).width, 0.0);
    h.state_mut().comment_prefs.set_width(CommentTool::Callout, 0.0);
    h.state_mut().comment_prefs.set_width(CommentTool::Rectangle, 0.0);
    assert_eq!(h.state().comment_prefs.style(CommentTool::Callout).width, 0.0);
    assert_eq!(h.state().comment_prefs.style(CommentTool::Rectangle).width, 0.5);
    let mut restored = crate::control::new_app();
    restored.apply_settings(&h.state().settings_snapshot());
    assert_eq!(restored.comment_prefs.style(CommentTool::TextBox).width, 0.0);
    assert_eq!(restored.comment_prefs.style(CommentTool::Callout).width, 0.0);
}

/// Moving a rectangle or an oval showed only its blue frame going while the shape stayed where
/// it was until release. Now the comment itself is drawn where it goes (the page beneath shows it
/// where it was until the move is applied), and the pointer frames change nothing.
#[test]
fn a_moved_comment_is_drawn_where_it_goes_before_release() {
    let _gpu = crate::gpu();
    let mut h = harness(|_| {});
    let id = h.state().views[0].id;
    for shape in
        [pdfcraft_engine::Shape::Rectangle { rect: [30.0, 30.0, 70.0, 70.0] }, pdfcraft_engine::Shape::Oval { rect: [230.0, 30.0, 270.0, 70.0] }]
    {
        let style = pdfcraft_engine::Style {
            color: [1.0, 0.0, 0.0],
            opacity: 1.0,
            width: 2.0,
            fill: Some([1.0, 0.0, 0.0]),
            fill_opacity: None,
            text_opacity: None,
            border_color: None,
            border_shape: pdfcraft_engine::BorderShape::Rectangle,
        };
        let ctx_h = h.ctx.clone();
        assert!(h.state_mut().apply_edit(
            &ctx_h,
            pdfcraft_engine::Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
                page: 0,
                shape,
                style,
                contents: String::new(),
                author: "Tester".into(),
            })
        ));
    }
    h.state_mut().views[0].session.comments.clear_selection();
    crate::support::settle(&mut h);
    let red = |v: [u8; 4]| v[0] > 200 && v[1] < 80 && v[2] < 80;
    for ((x, y), (tx, ty)) in [((50.0, 50.0), (150.0, 120.0)), ((250.0, 50.0), (200.0, 150.0))] {
        let generation = h.state().session.get(id).unwrap().edit_generation();
        let (from, to) = (at(&h, x, y), at(&h, tx, ty));
        // Beside the pointer, whose arrow is drawn over the page.
        let (was, goes) = (at(&h, x - 10.0, y - 10.0), at(&h, tx - 10.0, ty - 10.0));
        let px = |pixels: &image::RgbaImage, p: Pos2| pixels.get_pixel(p.x.round() as u32, p.y.round() as u32).0;
        h.hover_at(from);
        h.run_steps(1);
        h.drag_at(from);
        h.run_steps(1);
        for k in 1..=4 {
            h.hover_at(from + (to - from) * (k as f32 / 4.0));
            h.run_steps(1);
        }
        let mut shown = false;
        for _ in 0..100 {
            h.run_steps(2);
            let pixels = h.render().unwrap();
            if red(px(&pixels, goes)) {
                shown = true;
                assert!(red(px(&pixels, was)), "the page still shows it where it was");
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(shown, "the comment at ({x}, {y}) is drawn under the pointer before release");
        assert_eq!(h.state().session.get(id).unwrap().edit_generation(), generation, "pointer frames never edit the document");
        h.drop_at(to);
        h.run_steps(3);
        let doc = h.state().session.get(id).unwrap();
        assert_eq!(doc.can_undo(), Some("Move comment"));
        crate::support::settle(&mut h);
        let pixels = h.render().unwrap();
        assert!(!red(px(&pixels, was)), "once applied it is gone from where it was");
        assert!(red(px(&pixels, goes)), "and shown where it went");
    }
}

/// Fill & Sign's typed text (a Typewriter FreeText) is drawn where it goes while it is dragged
/// too, not only its frame (#4619).
#[test]
fn moved_fill_and_sign_text_is_drawn_where_it_goes_before_release() {
    let _gpu = crate::gpu();
    let mut h = harness(|_| {});
    h.execute("sign.fill.text");
    click_pt(&mut h, (40.0, 150.0));
    h.event(egui::Event::Text("WWWWWW".into()));
    h.run_steps(1);
    click(&mut h, pos2(350.0, 850.0)); // Outside the field: Enter now inserts a line break.
    h.run_steps(3);
    let a = comments(&h).into_iter().find(|a| a.contents.as_deref() == Some("WWWWWW")).expect("the typed text");
    assert!(a.fill_sign && a.subtype == "FreeText", "{a:?}");
    h.state_mut().quick_tool = QuickTool::Select;
    h.state_mut().views[0].session.comments.clear_selection();
    crate::support::settle(&mut h);
    let (x, y) = ((a.rect[0] + a.rect[2]) / 2.0, (a.rect[1] + a.rect[3]) / 2.0);
    let (dx, dy) = (120.0, -60.0);
    let (from, to) = (at(&h, x, y), at(&h, x + dx, y + dy));
    // Ink in the left of the box where it goes, inside its frame and away from the pointer, which
    // is drawn over the page.
    let goes = egui::Rect::from_two_pos(at(&h, a.rect[0] + dx, a.rect[1] + dy), at(&h, a.rect[2] + dx, a.rect[3] + dy)).shrink(3.0);
    let inked = |pixels: &image::RgbaImage| {
        (goes.left() as u32..(to.x - 12.0) as u32)
            .any(|px| (goes.top() as u32..goes.bottom() as u32).any(|py| pixels.get_pixel(px, py).0[..3].iter().all(|c| *c < 100)))
    };
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(from + (to - from) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    let mut shown = false;
    for _ in 0..100 {
        h.run_steps(2);
        if inked(&h.render().unwrap()) {
            shown = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(shown, "the typed text is drawn under the pointer before release");
    h.drop_at(to);
    h.run_steps(3);
}

/// A comment kept upright on a turned page (NoRotate on a /Rotate 90 page) is drawn where it goes
/// while it is dragged too: its layer is cut from the box it is drawn in, not from its /Rect.
#[test]
fn an_upright_comment_on_a_turned_page_is_drawn_where_it_goes_before_release() {
    let _gpu = crate::gpu();
    let pdf = String::from_utf8_lossy(TEXT_FIXTURE)
        .replace("/Contents 5 0 R /Resources", "/Contents 5 0 R /Rotate 90 /Annots [8 0 R] /Resources")
        .replace(
            "trailer",
            "8 0 obj << /Type /Annot /Subtype /Square /Rect [40 60 160 90] /F 20 /C [1 0 0] /IC [1 0 0] /BS << /W 2 >> >> endobj\ntrailer",
        )
        .into_bytes();
    let mut h = harness_with_bytes(pdf, |_| {});
    crate::support::settle(&mut h);
    let id = h.state().views[0].id;
    let drawn = {
        let app = h.state();
        let v = &app.views[0];
        let info = &app.session.get(v.id).unwrap().info;
        let a = info.annotations.iter().find(|a| a.subtype == "Square").expect("the square");
        assert!(a.no_rotate, "kept upright");
        let page = &info.pages[0];
        let xf = pdfcraft_ui_egui::canvas::PageXform {
            rect: v.page_screen_rect(0).expect("on screen"),
            rot: v.view.rotation,
            pw: page.width,
            ph: page.height,
        };
        xf.user_rect(info, 0, a.shown_rect(page.rotation))
    };
    let red = |v: [u8; 4]| v[0] > 200 && v[1] < 80 && v[2] < 80;
    let px = |pixels: &image::RgbaImage, p: Pos2| pixels.get_pixel(p.x.round() as u32, p.y.round() as u32).0;
    // Beside the pointer, which is drawn over the page.
    let (from, delta) = (drawn.center(), egui::vec2(160.0, 120.0));
    let was = pos2(drawn.left() + 6.0, drawn.center().y);
    let goes = was + delta;
    assert!(red(px(&h.render().unwrap(), was)), "drawn in its upright box");
    let generation = h.state().session.get(id).unwrap().edit_generation();
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(from + delta * (k as f32 / 4.0));
        h.run_steps(1);
    }
    let mut shown = false;
    for _ in 0..100 {
        h.run_steps(2);
        let pixels = h.render().unwrap();
        if red(px(&pixels, goes)) {
            shown = true;
            assert!(red(px(&pixels, was)), "the page still shows it where it was");
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(shown, "the upright comment is drawn under the pointer before release");
    assert_eq!(h.state().session.get(id).unwrap().edit_generation(), generation, "pointer frames never edit the document");
    h.drop_at(from + delta);
    h.run_steps(3);
}

// Contributor-original PDF: a Square, a Circle grouped with it (`/RT /Group`) that keeps its own
// author, text and colour, an ordinary reply and a reply to the Circle.
const GROUP_FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Annots [5 0 R 6 0 R 7 0 R 8 0 R] >> endobj
5 0 obj << /Type /Annot /Subtype /Square /Rect [20 20 100 80] /NM (sq-primary) /T (Alice) /Contents (Primary) /C [1 0 0] >> endobj
6 0 obj << /Type /Annot /Subtype /Circle /Rect [180 120 260 180] /NM (circ-sub) /T (Bob) /Contents (Subordinate) /C [0 0 1] /IRT 5 0 R /RT /Group >> endobj
7 0 obj << /Type /Annot /Subtype /Text /Rect [120 20 140 40] /NM (reply) /T (Carol) /Contents (Plain reply) /IRT 5 0 R >> endobj
8 0 obj << /Type /Annot /Subtype /Text /Rect [120 50 140 70] /NM (sub-reply) /T (Dan) /Contents (Reply to the subordinate) /IRT 6 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// ISO 32000-2 §12.5.6.2: a group is one comment, its primary's. The panel lists the Square
/// with both replies (the one to the Circle joins its thread) and not the Circle's own author
/// or text; clicking the Circle on the page selects the group, which is the Square's card.
#[test]
fn a_group_is_one_comment_in_the_panel_and_on_the_page() {
    let mut h = harness_with_bytes(GROUP_FIXTURE.to_vec(), |app| app.set_option(&egui::Context::default(), "panel", "comments").unwrap());
    // The panel names a card or reply row by its author, text and date.
    h.get_by_label_contains("Primary");
    h.get_by_label_contains("Plain reply");
    h.get_by_label_contains("Reply to the subordinate");
    assert_eq!(h.query_all_by_label_contains("Subordinate").count(), 0, "the subordinate's own text is ignored");
    assert_eq!(h.query_all_by_label_contains("Bob").count(), 0, "the subordinate's own author is ignored");
    let p = at(&h, 220.0, 150.0);
    click(&mut h, p);
    assert_eq!(h.state().views[0].session.comments.selection(), vec![(0, 0)], "clicking the Circle selects its group");
    h.state_mut().views[0].session.comments.clear_selection();
    let p = at(&h, 60.0, 50.0);
    click(&mut h, p);
    assert_eq!(h.state().views[0].session.comments.selection(), vec![(0, 0)]);
    // #4358 review: a right-click on the Circle selects the group too, so the menu is the
    // comment's, and a double-click there opens the group's pop-up window, which is the Square's
    // (comment.popups).
    h.state_mut().views[0].session.comments.clear_selection();
    let p = at(&h, 220.0, 150.0);
    h.hover_at(p);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed: true, modifiers: Default::default() });
    h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed: false, modifiers: Default::default() });
    h.run_steps(3);
    assert_eq!(h.state().views[0].session.comments.selection(), vec![(0, 0)], "a right-click on the Circle selects its group");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    h.state_mut().views[0].session.comments.clear_selection();
    crate::support::input::double_click(&mut h, p);
    h.run_steps(4);
    let c = comments(&h);
    let primary = c.iter().find(|a| a.contents.as_deref() == Some("Primary")).expect("the primary");
    assert_eq!(primary.popup.map(|w| w.open), Some(true), "a double-click on the Circle opens its group's window");
    assert_eq!(popup_windows(&h, "Alice").len(), 1, "one window, the Square's");
}

/// As in Acrobat desktop, Delete deletes the group member clicked and keeps the rest of its
/// group, although ISO 32000-2 §12.5.6.2 lists deletion among the operations that act on the
/// entire group. The Circle goes with its reply and the Square stays; the Square goes with its
/// reply and the Circle stays, a comment of its own.
#[test]
fn delete_deletes_the_group_member_clicked() {
    let shapes = |h: &Harness<'static, PdfCraftApp>| -> Vec<(String, bool)> {
        let mut out: Vec<(String, bool)> = comments(h).iter().filter(|a| a.subtype != "Text").map(|a| (a.subtype.clone(), a.is_comment())).collect();
        out.sort();
        out
    };
    let mut h = harness_with_bytes(GROUP_FIXTURE.to_vec(), |_| {});
    click_pt(&mut h, (220.0, 150.0));
    assert_eq!(h.state().views[0].session.comments.selection(), vec![(0, 0)], "the group is selected");
    h.key_press(egui::Key::Delete);
    h.run_steps(3);
    assert_eq!(shapes(&h), [("Square".to_string(), true)], "the Circle goes, the Square stays");
    let texts: Vec<String> = comments(&h).iter().filter_map(|a| a.contents.clone()).collect();
    assert!(!texts.iter().any(|c| c == "Reply to the subordinate") && texts.iter().any(|c| c == "Plain reply"), "{texts:?}");
    undo(&mut h);
    h.run_steps(3);

    click_pt(&mut h, (60.0, 50.0));
    h.key_press(egui::Key::Delete);
    h.run_steps(3);
    assert_eq!(shapes(&h), [("Circle".to_string(), true)], "the Square goes, the Circle stays as a comment of its own");
}

/// CodeRabbit on #4358: the member a click on the page picked is forgotten once the selection is
/// made anywhere else. After clicking the Circle, choosing the group's card in the Comments panel
/// selects the comment the card shows, the Square, so Delete deletes the Square, not the Circle
/// the page click remembered.
#[test]
fn choosing_the_groups_card_makes_delete_act_on_the_card_not_the_member_clicked() {
    let shapes = |h: &Harness<'static, PdfCraftApp>| -> Vec<String> {
        let mut out: Vec<String> = comments(h).iter().filter(|a| a.subtype != "Text").map(|a| a.subtype.clone()).collect();
        out.sort();
        out
    };
    let mut h = harness_with_bytes(GROUP_FIXTURE.to_vec(), |app| app.set_option(&egui::Context::default(), "panel", "comments").unwrap());
    click_pt(&mut h, (220.0, 150.0));
    assert_eq!(h.state().views[0].session.comments.primary(), Some((0, 0)), "the Circle's click selects the group");
    // The selected card, named by its author first.
    h.get_by(|n| n.role() == Role::TreeItem && n.label().is_some_and(|l| l.starts_with("Alice:"))).click();
    h.run_steps(3);
    assert_eq!(h.state().views[0].session.comments.primary(), Some((0, 0)), "the card selects the same group");
    assert_eq!(h.state().views[0].session.comments.selection().len(), 1, "only the group's card");
    h.key_press(egui::Key::Delete);
    h.run_steps(3);
    assert_eq!(shapes(&h), ["Circle".to_string()], "the card's comment, the Square, goes; the Circle stays");
}

/// One 300×200 page with a callout: text box [150 100 250 140], knee (120, 120) on the box's
/// left, arrow point (60, 60).
const CALLOUT_FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Annots [4 0 R] >> endobj
4 0 obj << /Type /Annot /Subtype /FreeText /IT /FreeTextCallout /Rect [50 50 250 140] /RD [100 50 0 0]
  /CL [60 60 120 120 150 120] /LE /OpenArrow /DA (/Helv 10 Tf 0 g) /Contents (Note) /F 4 >> endobj
trailer << /Root 1 0 R >>
%%EOF";

fn callout(h: &Harness<'static, PdfCraftApp>) -> Annotation {
    comments(h).into_iter().find(|a| a.subtype == "FreeText").expect("the callout")
}

fn near_pt(p: [f32; 2], x: f32, y: f32) -> bool {
    (p[0] - x).abs() < 1.5 && (p[1] - y).abs() < 1.5
}

fn last_edit(h: &Harness<'static, PdfCraftApp>) -> Option<String> {
    h.state().session.get(h.state().views[0].id).unwrap().can_undo().map(str::to_owned)
}

/// #1152: a selected callout's three parts change on their own, as in Acrobat ("Callout text
/// boxes have three parts: a text box, a knee line, and an end-point line. You can resize each
/// part by dragging a handle. The knee line can be resized in one direction only", Acrobat DC
/// Help, 11/28/2019, p. 357).
#[test]
fn a_callouts_arrow_knee_and_text_box_are_dragged_on_their_own() {
    let mut h = harness_with_bytes(CALLOUT_FIXTURE.to_vec(), |app| app.set_option(&egui::Context::default(), "quick", "select").unwrap());
    let c = callout(&h);
    assert_eq!((c.points.clone(), c.text_box), (vec![[60.0, 60.0], [120.0, 120.0]], Some([150.0, 100.0, 250.0, 140.0])));
    click_pt(&mut h, (200.0, 120.0));
    assert_eq!(h.state().views[0].session.comments.selection(), vec![(0, c.index)]);

    // The knee line is level, so the knee moves sideways only, and not into the box.
    drag_pt(&mut h, (120.0, 120.0), (95.0, 160.0));
    let c = callout(&h);
    assert!(near_pt(c.points[1], 95.0, 120.0) && near_pt(c.points[0], 60.0, 60.0), "{:?}", c.points);
    assert_eq!(last_edit(&h).as_deref(), Some("Reshape comment"));
    drag_pt(&mut h, (95.0, 120.0), (200.0, 120.0));
    assert!(near_pt(callout(&h).points[1], 138.0, 120.0), "stopped 12 pt short of the box: {:?}", callout(&h).points);
    drag_pt(&mut h, (138.0, 120.0), (110.0, 120.0));

    // The arrow point moves alone.
    drag_pt(&mut h, (60.0, 60.0), (40.0, 30.0));
    let c = callout(&h);
    assert!(near_pt(c.points[0], 40.0, 30.0) && near_pt(c.points[1], 110.0, 120.0), "{:?}", c.points);
    assert_eq!(c.text_box, Some([150.0, 100.0, 250.0, 140.0]));

    // The text box resizes from its own handles (not /Rect's); its left side, where the line meets
    // it, stays, so the line doesn't change.
    drag_pt(&mut h, (250.0, 100.0), (270.0, 80.0));
    let c = callout(&h);
    let tb = c.text_box.expect("a text box");
    assert!((tb[0] - 150.0).abs() < 1.5 && (tb[1] - 80.0).abs() < 1.5 && (tb[2] - 270.0).abs() < 1.5 && (tb[3] - 140.0).abs() < 1.5, "{tb:?}");
    assert!(near_pt(c.points[0], 40.0, 30.0) && near_pt(c.points[1], 110.0, 120.0), "{:?}", c.points);
    assert_eq!(last_edit(&h).as_deref(), Some("Resize comment"));

    // Dragged from inside, the box moves alone: the arrow stays, the knee comes along.
    drag_pt(&mut h, (200.0, 120.0), (220.0, 130.0));
    let c = callout(&h);
    let tb = c.text_box.expect("a text box");
    assert!((tb[0] - 170.0).abs() < 1.5 && (tb[3] - 150.0).abs() < 1.5, "{tb:?}");
    assert!(near_pt(c.points[0], 40.0, 30.0) && near_pt(c.points[1], 130.0, 130.0), "{:?}", c.points);

    // Dragged by its leader line, the whole callout moves.
    drag_pt(&mut h, (85.0, 80.0), (95.0, 80.0));
    let c = callout(&h);
    assert!(near_pt(c.points[0], 50.0, 30.0) && near_pt(c.points[1], 140.0, 130.0), "{:?}", c.points);
    assert_eq!(last_edit(&h).as_deref(), Some("Move comment"));

    // Each drag was one undo step.
    undo(&mut h);
    h.run_steps(2);
    assert!(near_pt(callout(&h).points[0], 40.0, 30.0));
}

/// #2137 review: a pop-up is edited under comment permission, not change permission: in a
/// document whose security allows comments but not changes, an open pop-up still takes typing
/// and closes.
#[test]
fn a_pop_up_is_editable_where_comments_are_allowed_but_changes_are_not() {
    let plain = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Annots [4 0 R 5 0 R] >> endobj
4 0 obj << /Type /Annot /Subtype /Text /Rect [20 150 40 170] /T (Ada) /Popup 5 0 R >> endobj
5 0 obj << /Type /Annot /Subtype /Popup /Parent 4 0 R /Rect [60 60 260 160] /Open true >> endobj
trailer << /Root 1 0 R >>
%%EOF";
    let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(plain.to_vec())).unwrap();
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
    let bytes = pdfcraft_cos::write_full(&doc, &Default::default()).unwrap();
    let mut h = harness(move |app| {
        app.open_bytes(&egui::Context::default(), "restricted.pdf", None, bytes).expect("opens");
        // Only the restricted document, so it is the first view.
        app.close_tab(0);
    });
    h.run_steps(6);
    let window = popup_rect(&h, "Ada");
    // Changes are forbidden, comments are not: the window's text field takes typing.
    let editable = h
        .query_all_by(|n| n.role() == Role::MultilineTextInput)
        .filter(|n| window.contains(n.rect().center()))
        .any(|n| !NodeT::accesskit_node(&n).is_read_only());
    assert!(editable, "the pop-up's text is editable under comment permission");
    let close = h.query_all_by_label("Close").find(|n| window.contains(n.rect().center())).expect("the pop-up's ×");
    close.click();
    h.run_steps(4);
    let note = comments(&h).into_iter().find(|a| a.subtype == "Text").expect("the note");
    assert_eq!(note.popup.map(|p| p.open), Some(false), "closed");
}

/// #2137 review: a pop-up lists the comment's text replies only, as the Comments panel does. A
/// review status (`/StateModel /Review`) and a checkmark (`/StateModel /Marked`, the private
/// reply Mark writes) are replies too, but not ones to read in the thread.
#[test]
fn a_pop_up_lists_text_replies_but_not_status_or_checkmark_replies() {
    let pdf = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Annots [4 0 R 5 0 R 6 0 R 7 0 R 8 0 R] >> endobj
4 0 obj << /Type /Annot /Subtype /Text /Rect [20 150 40 170] /T (Ada) /NM (note-1) /Contents (Please check) /Popup 5 0 R >> endobj
5 0 obj << /Type /Annot /Subtype /Popup /Parent 4 0 R /Rect [60 40 260 160] /Open true >> endobj
6 0 obj << /Type /Annot /Subtype /Text /Rect [20 150 40 170] /F 32 /T (Bob) /NM (reply-1) /IRT 4 0 R /Contents (Agreed with this) >> endobj
7 0 obj << /Type /Annot /Subtype /Text /Rect [20 150 40 170] /F 32 /T (Bob) /NM (reply-2) /IRT 4 0 R /State /Accepted /StateModel /Review /Contents (Accepted set by Bob) >> endobj
8 0 obj << /Type /Annot /Subtype /Text /Rect [20 150 40 170] /F 32 /T (Ada) /NM (reply-3) /IRT 4 0 R /State /Marked /StateModel /Marked /Contents (Marked set by Ada) >> endobj
trailer << /Root 1 0 R >>
%%EOF";
    let mut h = harness(move |app| {
        app.open_bytes(&egui::Context::default(), "thread.pdf", None, pdf.to_vec()).expect("opens");
        app.close_tab(0);
    });
    h.run_steps(6);
    let window = popup_rect(&h, "Ada");
    // Scroll the pop-up body so replies below the 120 pt initial height scroll into the window.
    h.hover_at(window.center());
    h.run_steps(1);
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -120.0),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
        source: egui::MouseWheelSource::Unknown,
    });
    h.run_steps(3);
    let in_window = |text: &str| h.query_all_by(|n| shows_text(n, text)).any(|n| window.contains(n.rect().center()));
    assert!(in_window("Agreed with this"), "the text reply");
    assert!(!in_window("Accepted set by Bob"), "not the review status");
    assert!(!in_window("Marked set by Ada"), "not the checkmark");
}

/// A Replace Text proposal: a strikeout and a caret grouped by `/IRT` and `/RT /Group`, with the
/// pop-up `open`. `caret_first` writes the caret as the group's primary, holding the replacement,
/// with the strikeout grouped under it; otherwise the strikeout is the primary and the
/// replacement is on the caret grouped under it, as PdfCraft used to write it.
fn replace_text_pdf(caret_first: bool, open: bool) -> Vec<u8> {
    let primary = if caret_first { 4 } else { 5 };
    let strike = format!(
        "<< /Type /Annot /Subtype /StrikeOut /Rect [40 100 140 120] /QuadPoints [40 120 140 120 40 100 140 100] /C [1 0 0] /T (Ada) /IT /StrikeOutTextEdit{} >>",
        if caret_first { " /IRT 4 0 R /RT /Group" } else { " /Popup 6 0 R" }
    );
    let caret = format!(
        "<< /Type /Annot /Subtype /Caret /Rect [135 95 145 105] /C [0 0 1] /T (Bob) /Contents (slow red){} >>",
        if caret_first { " /IT /Replace /Popup 6 0 R" } else { " /IRT 5 0 R /RT /Group" }
    );
    format!(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Annots [5 0 R 4 0 R 6 0 R] >> endobj
4 0 obj {caret} endobj
5 0 obj {strike} endobj
6 0 obj << /Type /Annot /Subtype /Popup /Parent {primary} 0 R /Rect [160 20 290 150] /Open {open} >> endobj
trailer << /Root 1 0 R >>
%%EOF",
    )
    .into_bytes()
}

/// A Replace Text proposal's window shows the replacement as the comment's text, and the other
/// member of the group is part of the comment, not a reply panel (ISO 32000-2 §12.5.6.2: a group's
/// members are one unit). Both layouts: the caret as the primary, and the strikeout as the primary
/// with the replacement on the grouped caret, as `main` writes it (#5688).
#[test]
fn a_replace_text_window_shows_the_replacement_as_its_text_not_as_a_reply() {
    for caret_first in [true, false] {
        let pdf = replace_text_pdf(caret_first, true);
        let mut h = harness(move |app| {
            app.open_bytes(&egui::Context::default(), "replace.pdf", None, pdf).expect("opens");
            app.close_tab(0);
        });
        h.run_steps(6);
        // The window is titled with the author of the annotation that has the pop-up.
        let window = popup_rect(&h, if caret_first { "Bob" } else { "Ada" });
        let boxes: Vec<String> = h
            .query_all_by(|n| n.role() == Role::MultilineTextInput && !n.is_hidden())
            .filter(|n| window.contains(n.rect().center()))
            .map(|n| n.value().unwrap_or_default())
            .collect();
        assert_eq!(boxes, ["slow red"], "caret first: {caret_first}: one box, the replacement");
    }
}

/// A locked Replace Text proposal, as `main` writes it (the strikeout primary and locked, the
/// replacement on the grouped caret), shows the replacement in its window read-only: locking the
/// comment locks the text the window shows for it.
#[test]
fn a_locked_replace_text_window_shows_the_replacement_read_only() {
    let pdf = String::from_utf8(replace_text_pdf(false, true))
        .unwrap()
        .replace("/IT /StrikeOutTextEdit /Popup", "/IT /StrikeOutTextEdit /F 128 /Popup")
        .into_bytes();
    let mut h = harness(move |app| {
        app.open_bytes(&egui::Context::default(), "replace.pdf", None, pdf).expect("opens");
        app.close_tab(0);
    });
    h.run_steps(6);
    let window = popup_rect(&h, "Ada");
    let inside = |n: &egui_kittest::Node<'_>| window.contains(n.rect().center());
    let editable = h.query_all_by(|n| n.role() == Role::MultilineTextInput).filter(inside).any(|n| !NodeT::accesskit_node(&n).is_read_only());
    assert!(!editable, "no box to type in while the comment is locked");
    assert!(h.query_all_by_label("slow red").any(|n| inside(&n)), "the replacement is shown");
}

/// A double-click on a Replace Text proposal opens the group's one window, the primary's, in
/// either layout. (A double-click on the member grouped under it opens the same
/// window once members are picked on the page; main hit-tests only the primary.)
#[ignore = "known gap of the egui-to-craft port: see docs/port-known-gaps.md"]
#[test]
fn a_double_click_on_a_replace_text_opens_its_one_window() {
    for (caret_first, on_caret) in [(true, true), (false, false)] {
        let pdf = replace_text_pdf(caret_first, false);
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_eframe(move |cc| {
            crate::support::install_runtime(&cc.egui_ctx);
            {
                let mut app = crate::control::new_app();
                app.set_option(&egui::Context::default(), "language", "en").unwrap();
                app.open_bytes(&egui::Context::default(), "replace.pdf", None, pdf.clone()).expect("opens");
                app.set_option(&egui::Context::default(), "left", "closed").unwrap();
                app
            }
        });
        h.run_steps(4);
        crate::support::settle(&mut h);
        h.run_steps(45);
        let target = if on_caret { (140.0, 98.0) } else { (60.0, 110.0) };
        let p = at(&h, target.0, target.1);
        h.hover_at(p);
        h.run_steps(1);
        for _ in 0..2 {
            h.drag_at(p);
            h.run_steps(1);
            h.drop_at(p);
            h.run_steps(1);
        }
        h.run_steps(4);
        let primary_subtype = if caret_first { "Caret" } else { "StrikeOut" };
        let c = comments(&h);
        let primary = c.iter().find(|a| a.subtype == primary_subtype).expect("the primary");
        assert_eq!(primary.popup.map(|w| w.open), Some(true), "caret first: {caret_first}, on the caret: {on_caret}: the group's window opened");
        let author = if caret_first { "Bob" } else { "Ada" };
        assert_eq!(popup_windows(&h, author).len(), 1, "one window");
    }
}

/// A double-click on a sticky note opens its pop-up window on the page, where its text is read
/// and edited; the title bar drags it (saved as the pop-up's place, one undo step); × closes it
/// (comment.popups).
#[test]
fn a_double_click_opens_a_notes_pop_up_which_drags_and_closes() {
    // Frames a sixtieth apart, so two clicks land within egui's double-click time.
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "text.pdf", None, TEXT_FIXTURE.to_vec()).expect("opens");
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "author", "Tester").unwrap();
            app.set_option(&egui::Context::default(), "quick", "note").unwrap();
            app.set_option(&egui::Context::default(), "zoom", "150").unwrap();
            app
        }
    });
    h.run_steps(4);
    crate::support::settle(&mut h);
    click_pt(&mut h, (60.0, 160.0));
    field(&h, "Add a comment").type_text("Please check");
    h.run_steps(2);
    h.get_by_label("Post").click();
    h.run_steps(3);
    let note = comments(&h).into_iter().find(|a| a.subtype == "Text").expect("the note");
    assert_eq!(note.popup.map(|p| p.open), Some(false));
    assert!(popup_windows(&h, "Tester").is_empty(), "closed pop-ups are not shown");
    // Well after the Post click, so egui does not count the next clicks with it.
    h.run_steps(45);

    // Double-click the icon.
    let icon = at(&h, (note.rect[0] + note.rect[2]) / 2.0, (note.rect[1] + note.rect[3]) / 2.0);
    h.hover_at(icon);
    h.run_steps(1);
    for _ in 0..2 {
        h.drag_at(icon);
        h.run_steps(1);
        h.drop_at(icon);
        h.run_steps(1);
    }
    h.run_steps(4);
    let note = comments(&h).into_iter().find(|a| a.subtype == "Text").expect("the note");
    assert_eq!(note.popup.map(|p| p.open), Some(true), "the pop-up opened");
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Open comment pop-up"));
    let window = popup_rect(&h, "Tester");
    // Its title names the kind of comment and its author.
    h.get_by_role_and_label(Role::Heading, "Note – Tester");

    // Drag the title band 80 px right and 40 px down: the pop-up's place on the page moves, and
    // is saved once, when the drag is released.
    let before = note.popup.unwrap().rect;
    let bar = pos2(window.left() + 30.0, window.top() + 12.0);
    let (from, to) = (bar, bar + egui::vec2(80.0, 40.0));
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(from + (to - from) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    assert_eq!(comments(&h).into_iter().find(|a| a.subtype == "Text").unwrap().popup.unwrap().rect, before, "nothing is saved before the release");
    h.drop_at(to);
    h.run_steps(3);
    let note = comments(&h).into_iter().find(|a| a.subtype == "Text").expect("the note");
    let after = note.popup.unwrap().rect;
    // 80 × 40 screen pixels is about 20 × 10 pt at this zoom.
    assert!(after[0] > before[0] + 10.0 && after[1] < before[1] - 5.0, "moved right and down on the page: {before:?} -> {after:?}");
    assert!(((after[2] - after[0]) - (before[2] - before[0])).abs() < 0.5, "the same width");
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Move comment pop-up"));

    // Close it.
    let window = popup_rect(&h, "Tester");
    let close = h.query_all_by_label("Close").map(|n| n.rect().center()).find(|c| window.contains(*c)).expect("the window's ×");
    click(&mut h, close);
    h.run_steps(4);
    let note = comments(&h).into_iter().find(|a| a.subtype == "Text").expect("the note");
    assert_eq!(note.popup.map(|p| p.open), Some(false));
    assert!(popup_windows(&h, "Tester").is_empty());
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Close comment pop-up"));
}

// Contributor-original PDF: two notes sharing one /NM (as PyMuPDF names them), both with an open
// pop-up; the second has a reply by reference and a reply to that reply, which has no /NM.
const POPUPS_SHARING_A_NAME: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Annots [4 0 R 5 0 R 6 0 R 7 0 R 8 0 R 9 0 R] >> endobj
4 0 obj << /Type /Annot /Subtype /Text /Rect [20 160 40 180] /NM (fitz-A0) /T (Alpha) /Contents (First note) /Popup 5 0 R /Open true >> endobj
5 0 obj << /Type /Annot /Subtype /Popup /Rect [60 110 160 190] /Parent 4 0 R /Open true >> endobj
6 0 obj << /Type /Annot /Subtype /Text /Rect [20 40 40 60] /NM (fitz-A0) /T (Beta) /Contents (Second note) /Popup 7 0 R /Open true >> endobj
7 0 obj << /Type /Annot /Subtype /Popup /Rect [180 -250 290 90] /Parent 6 0 R /Open true >> endobj
8 0 obj << /Type /Annot /Subtype /Text /Rect [20 40 40 60] /NM (fitz-A1) /IRT 6 0 R /T (Gamma) /Contents (Reply by reference) >> endobj
9 0 obj << /Type /Annot /Subtype /Text /Rect [20 40 40 60] /IRT 8 0 R /T (Delta) /Contents (Reply to the reply) >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// A pop-up reads its comment's thread by `/IRT` reference, as the Comments panel does: the
/// replies show once, in the second note's window only, at any depth, though both notes share
/// one `/NM`.
#[test]
fn a_pop_up_lists_its_replies_by_reference_when_names_repeat() {
    let h = harness_with_bytes(POPUPS_SHARING_A_NAME.to_vec(), |_| {});
    let windows = [popup_rect(&h, "Alpha"), popup_rect(&h, "Beta")];
    let second = windows[1];
    for text in ["Reply by reference", "Reply to the reply"] {
        // The Comments panel lists the thread too; count only what the windows show.
        let shown: Vec<Pos2> =
            h.query_all_by(|n| shows_text(n, text)).map(|n| n.rect().center()).filter(|c| windows.iter().any(|w| w.contains(*c))).collect();
        assert_eq!(shown.len(), 1, "{text:?} shows in one window: {shown:?}");
        assert!(shown.iter().all(|c| second.contains(*c)), "{text:?} is in the second note's window");
    }
}

// Contributor-original PDF: one blue note with its pop-up open beside it.
const BLUE_NOTE_WITH_OPEN_POP_UP: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Annots [4 0 R 5 0 R] >> endobj
4 0 obj << /Type /Annot /Subtype /Text /Rect [20 160 40 180] /C [0 0 1] /T (Alpha) /Contents (A note) /Popup 5 0 R /Open true >> endobj
5 0 obj << /Type /Annot /Subtype /Popup /Rect [120 20 260 120] /Parent 4 0 R /Open true >> endobj
trailer << /Root 1 0 R >>
%%EOF";

#[test]
fn an_uncoloured_notes_popup_uses_the_yellow_note_default() {
    let _gpu = crate::gpu();
    let pdf = String::from_utf8_lossy(BLUE_NOTE_WITH_OPEN_POP_UP).replace("/C [0 0 1]", "");
    let mut h = harness_with_bytes(pdf.into_bytes(), |_| {});
    let window = popup_rect(&h, "Alpha");
    let pixels = h.render().unwrap();
    let x0 = window.left().max(0.0) as u32;
    let x1 = window.right().min(pixels.width() as f32) as u32;
    let y0 = window.top().max(0.0) as u32;
    let y1 = (window.top() + 16.0).min(pixels.height() as f32) as u32;
    let yellow = (y0..y1).any(|y| {
        (x0..x1).any(|x| {
            let [r, g, b, _] = pixels.get_pixel(x, y).0;
            r > 200 && g > 150 && b < 150
        })
    });
    assert!(yellow, "a note without a colour has a yellow popup header");
}

/// A note with an open pop-up and a thread of replies: "Second" and "Third" answer the note,
/// "Second, first answer" answers "Second" (`/IRT` to a reply).
const NOTE_WITH_A_THREAD: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Annots [4 0 R 5 0 R 6 0 R 7 0 R 8 0 R] >> endobj
4 0 obj << /Type /Annot /Subtype /Text /Rect [20 160 40 180] /C [1 0.85 0.2] /T (Alpha) /Contents (First) /Popup 5 0 R /Open true >> endobj
5 0 obj << /Type /Annot /Subtype /Popup /Rect [60 -250 280 150] /Parent 4 0 R /Open true >> endobj
6 0 obj << /Type /Annot /Subtype /Text /Rect [20 160 40 180] /T (Beta) /Contents (Second) /IRT 4 0 R /F 30 >> endobj
7 0 obj << /Type /Annot /Subtype /Text /Rect [20 160 40 180] /T (Beta) /Contents (Second, first answer) /IRT 6 0 R /F 30 >> endobj
8 0 obj << /Type /Annot /Subtype /Text /Rect [20 160 40 180] /T (Beta) /Contents (Third) /IRT 4 0 R /F 30 >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// A pop-up window (comment.popup-windows) is laid out as the readers' are: a title bar with the
/// comment's kind and author; the comment's text; its replies as panels in thread order, an
/// answer to a reply inside that reply's panel; and a Reply button under each, which opens a box
/// that posts an answer to that comment or reply.
#[test]
fn a_pop_up_threads_its_replies_and_takes_a_new_one() {
    let mut h = harness_with_bytes(NOTE_WITH_A_THREAD.to_vec(), |_| {});
    let window = popup_rect(&h, "Alpha");
    h.get_by_role_and_label(Role::Heading, "Note – Alpha");
    // Each reply's text box, inside the window.
    let rect = |h: &Harness<'static, PdfCraftApp>, text: &str| {
        let found: Vec<egui::Rect> = h.query_all_by(|n| shows_text(n, text)).map(|n| n.rect()).filter(|r| window.contains_rect(*r)).collect();
        assert_eq!(found.len(), 1, "{text:?} once in the window: {found:?}");
        found[0]
    };
    let (second, answer, third) = (rect(&h, "Second"), rect(&h, "Second, first answer"), rect(&h, "Third"));
    assert!(second.bottom() < answer.top() && answer.bottom() < third.top(), "thread order: {second:?} {answer:?} {third:?}");
    let reply_panel = |r: egui::Rect| {
        h.query_all_by(|n| matches!(n.role(), Role::Group | Role::GenericContainer) && !n.is_hidden() && n.raw_bounds().is_some())
            .map(|n| n.rect())
            .filter(|&g| g.left() >= window.left() - 1.0 && g.right() <= window.right() + 1.0 && g != window && g.contains_rect(r))
            .min_by(|a, b| (a.width() * a.height()).total_cmp(&(b.width() * b.height())))
            .expect("the reply's panel")
    };
    let second_panel = reply_panel(second);
    assert!(
        answer.left() > second.left() + 4.0 && second_panel.contains_rect(answer),
        "the answer sits inside its reply's panel: {second:?} {answer:?} panel={second_panel:?}"
    );
    assert!((third.left() - second.left()).abs() < 1.0, "replies to the note line up: {second:?} {third:?}");
    // The Reply buttons, top to bottom: the note's, then each reply's.
    let replies = |h: &Harness<'static, PdfCraftApp>| {
        let mut b: Vec<egui::Rect> =
            h.query_all_by(|n| n.role() == Role::Button && n.label().as_deref() == Some("Reply")).map(|n| n.rect()).collect();
        b.sort_by(|x, y| x.top().total_cmp(&y.top()));
        b
    };
    assert_eq!(replies(&h).len(), 4, "one under the note and each reply");

    // The note's Reply: Enter starts a new line in the box, as in the readers' pop-ups; Post posts.
    let before = comments(&h).len();
    let at = replies(&h)[0].center();
    click(&mut h, at);
    field(&h, "Reply").click();
    h.run_steps(1);
    field(&h, "Reply").type_text("Fourth");
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert_eq!(comments(&h).len(), before, "Enter does not post");
    h.event(egui::Event::Text("line two".into()));
    h.run_steps(2);
    h.get_by_label("Post").click();
    h.run_steps(3);
    let all = comments(&h);
    let note = all.iter().find(|a| a.contents.as_deref() == Some("First")).expect("the note");
    let reply = all.iter().find(|a| a.contents.as_deref() == Some("Fourth\nline two")).expect("the reply was added, on two lines");
    assert_eq!(reply.parent, Some((note.page, note.index)));
    assert_eq!(reply.author.as_deref(), Some("Tester"));
    assert!(h.query_all_by(|n| n.placeholder() == Some("Reply")).next().is_none(), "the box closes");

    // A reply's own Reply answers that reply; Ctrl+Enter posts too, as in the comment composer.
    let second_key = all.iter().find(|a| a.contents.as_deref() == Some("Second")).map(|a| (a.page, a.index)).expect("the reply");
    let at = replies(&h)[1].center();
    click(&mut h, at);
    field(&h, "Reply").click();
    h.run_steps(1);
    field(&h, "Reply").type_text("Fifth");
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::Enter);
    h.run_steps(3);
    let fifth = comments(&h).into_iter().find(|a| a.contents.as_deref() == Some("Fifth")).expect("Ctrl+Enter posts");
    assert_eq!(fifth.parent, Some(second_key), "it answers the reply");

    // Cancel closes the box without posting.
    let before = comments(&h).len();
    let at = replies(&h)[0].center();
    click(&mut h, at);
    field(&h, "Reply").click();
    h.run_steps(1);
    field(&h, "Reply").type_text("Never mind");
    h.run_steps(2);
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert_eq!(comments(&h).len(), before);
    assert!(h.query_all_by(|n| n.placeholder() == Some("Add a reply")).next().is_none(), "the box closes");
}

/// A pop-up window's text `text` (a comment's or a reply's): the comment's box, or a reply's read-only text.
fn shows_text(n: &egui_kittest::kittest::AccessKitNode<'_>, text: &str) -> bool {
    matches!(n.role(), Role::MultilineTextInput | Role::Label) && (n.value().as_deref() == Some(text) || n.label().as_deref() == Some(text))
}

/// On a page turned a quarter (View ▸ Rotate View), a resized pop-up window stays where it is on
/// screen and shows the size it was dragged to. Rects are compared in screen points: a pop-up
/// keeps its size on screen at any zoom, and the app maps the result back to user space.
#[test]
fn a_pop_up_resized_on_a_turned_page_stays_in_place() {
    let mut h = harness_with_bytes(BLUE_NOTE_WITH_OPEN_POP_UP.to_vec(), |app| app.views[0].view.rotation = 90);
    let window = popup_rect(&h, "Alpha");
    let corner = window.right_bottom() - egui::vec2(3.0, 3.0);
    drag(&mut h, corner, corner + egui::vec2(80.0, 60.0));
    h.run_steps(3);
    let resized = popup_rect(&h, "Alpha");
    assert!((resized.left_top() - window.left_top()).length() < 1.0, "in the same place on screen: {window:?} -> {resized:?}");
    assert!((resized.width() - window.width() - 80.0).abs() < 3.0, "80 px wider: {window:?} -> {resized:?}");
    // Taller by the drag, less what its text had already stretched it.
    assert!(resized.height() > window.height() + 50.0, "taller: {window:?} -> {resized:?}");
}

/// #2137 review: the composer's Post, clicked while a pop-up's box holds unsaved text, also
/// saves that text as the box loses the focus. Both land in one frame and both are kept.
#[test]
fn posting_from_the_composer_keeps_a_pop_ups_unsaved_text() {
    let mut h = harness_with_bytes(BLUE_NOTE_WITH_OPEN_POP_UP.to_vec(), |app| app.set_option(&egui::Context::default(), "quick", "note").unwrap());
    // Low on the left, so the composer stays clear of the window.
    click_pt(&mut h, (30.0, 40.0));
    // The composer opens with its editor focused.
    h.run_steps(2);
    h.event(egui::Event::Text("New note".into()));
    h.run_steps(2);
    let box_at = h.query_all_by(|n| shows_text(n, "A note")).next().expect("the pop-up's box").rect().right_center() - egui::vec2(4.0, 0.0);
    click(&mut h, box_at);
    h.event(egui::Event::Text(" edited".into()));
    h.run_steps(2);
    assert!(h.state().views[0].session.comments.composer.is_some(), "the composer is still open");
    let post = h.get_by_label("Post").rect().center();
    click(&mut h, post);
    h.run_steps(3);
    let all = comments(&h);
    assert!(
        all.iter().any(|a| a.contents.as_deref() == Some("New note")),
        "the composer's note: {:?}",
        all.iter().map(|a| &a.contents).collect::<Vec<_>>()
    );
    assert!(
        all.iter().any(|a| a.contents.as_deref() == Some("A note edited")),
        "the pop-up's text: {:?}",
        all.iter().map(|a| &a.contents).collect::<Vec<_>>()
    );
}

/// #2137 review: what a pop-up holds for a comment (unsaved text) follows that comment when a
/// deletion moves it, and is dropped when an undo or a page edit leaves its (page, index) naming
/// another; other edits keep it. The drafts are held under the key of the comment whose window
/// they belong to (`popup_drafts`); the typed text is what the window's box shows.
#[test]
fn a_pop_ups_unsaved_work_follows_its_comment_or_is_dropped() {
    use pdfcraft_engine::Edit;
    // The note with the pop-up is the second comment on its page, after one without a pop-up.
    let pdf = String::from_utf8_lossy(NOTE_WITH_A_THREAD)
        .replace("/Annots [4 0 R", "/Annots [9 0 R 4 0 R")
        .replace("trailer", "9 0 obj << /Type /Annot /Subtype /Text /Rect [200 20 220 40] /T (Zed) /Contents (Earlier) >> endobj\ntrailer")
        .into_bytes();
    let mut h = harness_with_bytes(pdf, |_| {});
    let key = |h: &Harness<'static, PdfCraftApp>, text: &str| {
        comments(h).into_iter().find(|a| a.contents.as_deref() == Some(text)).map(|a| (a.page, a.index)).expect(text)
    };
    let tag = |h: &Harness<'static, PdfCraftApp>| {
        let drafts = &h.state().views[0].session.comments.popup_drafts;
        if drafts.len() == 1 { drafts.keys().next().copied() } else { None }
    };
    let shows = |h: &Harness<'static, PdfCraftApp>, text: &str| h.query_all_by(|n| shows_text(n, text)).next().is_some();
    let type_into_the_note = |h: &mut Harness<'static, PdfCraftApp>, current: &str, typed: &str| {
        let box_at = h.query_all_by(|n| shows_text(n, current)).next().expect("the pop-up's box").rect().right_center() - egui::vec2(4.0, 0.0);
        click(h, box_at);
        h.event(egui::Event::Text(typed.into()));
        h.run_steps(2);
    };
    let (earlier, second) = (key(&h, "Earlier"), key(&h, "Second"));
    let note = key(&h, "First");
    assert_eq!(tag(&h), Some(note), "the drafts belong to the open window's comment");
    type_into_the_note(&mut h, "First", " more");
    assert!(shows(&h, "First more"), "the typed text is in the box");

    // Another edit, on another comment: the drafts stay.
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().apply_edit(&ctx_h, Edit::SetAnnotationContents { page: second.0, index: second.1, text: "Second!".into() }));
    h.run_steps(2);
    assert_eq!(tag(&h), Some(note));
    assert!(shows(&h, "First more"), "the typed text stays");

    // Deleting the comment before it moves it up: the drafts go with it.
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().apply_edit(&ctx_h, Edit::DeleteAnnotation { page: earlier.0, index: earlier.1 }));
    h.run_steps(2);
    let moved = key(&h, "First");
    assert_ne!(moved, note, "the note moved up");
    assert_eq!(tag(&h), Some(moved), "the drafts follow it");
    assert!(shows(&h, "First more"), "with the text typed");

    // Undo puts the comment back: the drafts are dropped rather than left on another comment.
    undo(&mut h);
    h.run_steps(3);
    assert!(!shows(&h, "First more") && shows(&h, "First"), "the unsaved text is dropped");

    // A page inserted before it: the comment's (page, index) now names nothing of it.
    type_into_the_note(&mut h, "First", " again");
    assert!(shows(&h, "First again"));
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().apply_edit(&ctx_h, Edit::InsertBlankPage { at: 0, width: 300.0, height: 200.0 }));
    h.run_steps(3);
    assert!(!shows(&h, "First again"), "the unsaved text is dropped");
}

/// #2137 review: text typed into a pop-up and not yet saved is saved when × closes the window:
/// the click takes the focus from the box and closes it in one frame, one undo step for both.
#[test]
fn closing_a_pop_up_saves_its_typed_text() {
    let mut h = harness_with_bytes(BLUE_NOTE_WITH_OPEN_POP_UP.to_vec(), |_| {});
    let box_at = h.query_all_by(|n| shows_text(n, "A note")).next().expect("the pop-up's box").rect().right_center() - egui::vec2(4.0, 0.0);
    click(&mut h, box_at);
    h.event(egui::Event::Text(" for you".into()));
    h.run_steps(2);
    let window = popup_rect(&h, "Alpha");
    let close = h.query_all_by_label("Close").map(|n| n.rect().center()).find(|c| window.contains(*c)).expect("the window's ×");
    click(&mut h, close);
    h.run_steps(3);
    let note = comments(&h).into_iter().find(|a| a.subtype == "Text").expect("the note");
    assert_eq!(note.contents.as_deref(), Some("A note for you"), "the typed text is saved");
    assert_eq!(note.popup.map(|p| p.open), Some(false), "and the window closed");
    assert!(popup_windows(&h, "Alpha").is_empty());
    undo(&mut h);
    h.run_steps(2);
    let note = comments(&h).into_iter().find(|a| a.subtype == "Text").expect("the note");
    assert_eq!((note.contents.as_deref(), note.popup.map(|p| p.open)), (Some("A note"), Some(true)), "one undo step for both");
}

#[test]
fn freehand_highlighter_draws_translucent_yellow_ink() {
    let mut h = harness(|app| app.set_option(&egui::Context::default(), "quick", "freehand_highlight").unwrap());
    drag_pt(&mut h, (40.0, 100.0), (240.0, 100.0));

    let c = comments(&h);
    assert_eq!(c.len(), 1, "{c:?}");
    assert_eq!(c[0].subtype, "Ink");
    assert_eq!(c[0].intent.as_deref(), Some("FreehandHighlight"));
    let props = h.state().session.get(h.state().views[0].id).unwrap().comment_props(0, c[0].index).expect("ink properties");
    assert_eq!(props.color, Some([1.0, 0.94, 0.0]));
    assert!((props.opacity - 0.35).abs() < 0.001);
    assert_eq!(props.width, Some(14.0));
    assert_eq!(h.state().quick_tool, QuickTool::Comment(pdfcraft_ui_egui::comments::CommentTool::FreehandHighlight));
    h.state_mut().comment_prefs.set_width(pdfcraft_ui_egui::comments::CommentTool::FreehandHighlight, 100.0);
    drag_pt(&mut h, (40.0, 160.0), (240.0, 160.0));
    let c = comments(&h);
    assert_eq!(c.len(), 2);
    let props = h.state().session.get(h.state().views[0].id).unwrap().comment_props(0, c.iter().map(|a| a.index).max().unwrap()).unwrap();
    assert_eq!(props.width, Some(100.0));
    assert!(h.state().views[0].session.comments.selection().is_empty());
}

/// The rail drawer's search field: a `TextField::search`, which reports a text input named for the
/// search (the framework has no separate search role).
fn drawer_search<'a>(h: &'a Harness<'static, PdfCraftApp>) -> Option<egui_kittest::Node<'a>> {
    h.query_all_by(|n| n.role() == Role::TextInput && n.label().is_some_and(|l| l.to_lowercase().contains("search"))).next()
}

#[test]
fn hotbar_picker_adds_and_runs_freehand_highlight() {
    let mut h = harness(|_| {});
    h.get_by_label("Edit toolbar").click();
    h.run_steps(3);
    drawer_search(&h).expect("the drawer's search field").click();
    h.run_steps(2);
    drawer_search(&h).expect("the drawer's search field").type_text("freehand");
    h.run_steps(3);
    let drag_to_rail = |h: &mut Harness<'static, PdfCraftApp>, label: &str| {
        let from = h.get_by_role_and_label(Role::ListItem, label).rect().center();
        let opener = h.get_by_label("Edit toolbar").rect().center();
        // A new slot is created by dropping on the leading third of an existing slot.
        let slot = h
            .query_all_by_role_and_label(Role::Button, "Select")
            .find(|button| (button.rect().center().x - opener.x).abs() < 8.0)
            .expect("the rail's Select slot")
            .rect();
        let to = egui::pos2(slot.center().x, slot.top() + slot.height() * 0.1);
        drag(h, from, to);
        h.run_steps(3);
    };
    drag_to_rail(&mut h, "Freehand-Highlight");
    drag_to_rail(&mut h, "Draw freehand");
    // The drawer stays open after adding a tool; the opener closes it.
    assert!(drawer_search(&h).is_some(), "the drawer stays open");
    h.get_by_label("Edit toolbar").click();
    h.run_steps(3);
    // The slots are kept across a restart.
    let saved = h.state().settings_snapshot();
    drop(h);
    let mut h = harness(move |app| app.apply_settings(&saved));
    // The new tool runs from its slot: it arms the freehand highlighter, which draws ink with it.
    h.query_all_by_role_and_label(Role::Button, "Freehand-Highlight").last().expect("the slot after a restart").click();
    h.run_steps(3);
    assert_eq!(h.state().quick_tool, QuickTool::Comment(pdfcraft_ui_egui::comments::CommentTool::FreehandHighlight));
    drag_pt(&mut h, (40.0, 100.0), (240.0, 100.0));
    let c = comments(&h);
    assert_eq!((c.len(), c[0].intent.as_deref()), (1, Some("FreehandHighlight")), "{c:?}");
    h.query_all_by_role_and_label(Role::Button, "Draw freehand").last().expect("the second slot").click();
    h.run_steps(3);
    assert_eq!(h.state().quick_tool, QuickTool::Comment(pdfcraft_ui_egui::comments::CommentTool::Ink));
}

fn edit_composer_preserves_only_typewriter_whitespace(kind: u8) {
    let shots = std::env::var_os("PDFCRAFT_COMPOSER_SHOTS").is_some();
    let _gpu = shots.then(crate::gpu);
    use pdfcraft_engine::{Edit, NewAnnotation, NoteIcon, Shape, Style};
    let mut h = harness(move |app| {
        app.set_option(&egui::Context::default(), "layout", "single").unwrap();
        app.set_option(&egui::Context::default(), "panel", "comments").unwrap();
        app.quick_tool = QuickTool::Select;
        // Both pages use index 0, with opposite intents. The lookup must use page AND index.
        for page in 0..2 {
            let typewriter = if page == 0 { kind != 0 } else { kind == 0 };
            let shape = if typewriter {
                Shape::Typewriter { rect: [40.0, 40.0, 180.0, 90.0], font_size: 10.0 }
            } else if kind == 2 {
                Shape::Note { at: [80.0, 80.0], icon: NoteIcon::Comment }
            } else {
                Shape::TextBox { rect: [40.0, 40.0, 180.0, 90.0], font_size: 10.0 }
            };
            let style = Style::default_for(&shape);
            assert!(app.apply_edit(
                &egui::Context::default(),
                Edit::AddAnnotation(NewAnnotation { page, shape, style, contents: "Original".into(), author: "Tester".into() })
            ));
        }
    });
    h.state_mut().views[0].go_to_page(1);
    h.run_steps(4);
    support::settle(&mut h);
    let point = {
        let s = h.state();
        let v = &s.views[0];
        let doc = s.session.get(v.id).unwrap();
        let a = doc.info.annotations.iter().find(|a| a.page == 1 && a.index == 0).unwrap();
        let page = v.page_screen_rect(1).expect("the second page is visible");
        let info = &doc.info.pages[1];
        let p = info.user_to_view((a.rect[0] + a.rect[2]) / 2.0, (a.rect[1] + a.rect[3]) / 2.0);
        page.min + egui::vec2(p[0], p[1]) * (page.width() / info.width)
    };
    support::input::double_click(&mut h, point);
    h.run_steps(3);
    if kind == 2 {
        // A double-click on a sticky note opens its pop-up window, as in Acrobat; its edit
        // composer is the page menu's "Edit text…".
        let popup_open = |h: &Harness<'static, PdfCraftApp>| {
            let s = h.state();
            let doc = s.session.get(s.views[0].id).unwrap();
            doc.info.annotations.iter().find(|a| a.page == 1 && a.index == 0).unwrap().popup.is_some_and(|p| p.open)
        };
        assert!(h.state().views[0].session.comments.composer.is_none(), "a note's double-click opens no composer");
        assert!(popup_open(&h), "a note's double-click opens its pop-up");
        h.hover_at(point);
        h.run_steps(1);
        h.event(egui::Event::PointerButton { pos: point, button: egui::PointerButton::Secondary, pressed: true, modifiers: Default::default() });
        h.event(egui::Event::PointerButton { pos: point, button: egui::PointerButton::Secondary, pressed: false, modifiers: Default::default() });
        h.run_steps(3);
        h.get_all_by_label("Edit text…").last().unwrap().click();
        h.run_steps(3);
    }
    let c = h.state().views[0].session.comments.composer.as_ref().expect("the edit composer is open");
    assert_eq!(c.page, 1);
    assert_eq!(c.kind, pdfcraft_ui_egui::comments::ComposerKind::Edit(0));
    h.input_mut().events.push(egui::Event::Text(" edited\n\n".into()));
    h.step();
    h.run_steps(2);
    assert_eq!(h.state().views[0].session.comments.composer.as_ref().unwrap().text, "Original edited\n\n");
    if shots {
        h.render().unwrap().save(format!("/tmp/5954-composer-{kind}.png")).unwrap();
    }
    h.get_all_by_label("Save").last().unwrap().click();
    h.run_steps(3);
    assert!(h.state().views[0].session.comments.composer.is_none());
    let id = h.state().views[0].id;
    let bytes = h.state().session.save_bytes(id).unwrap();
    // Check the raw saved string: ordinary-comment reader trimming must not hide a bad post.
    let cos = pdfcraft_cos::Document::open(bytes.clone()).unwrap();
    let pages = pdfcraft_model::pages(&cos);
    let annots = cos.resolve(pages[1].dict.get(b"Annots").unwrap());
    let annot = cos.resolve(&annots.as_array().unwrap()[0]);
    let contents = cos.resolve(annot.as_dict().unwrap().get(b"Contents").unwrap());
    let expected = if kind == 0 { "Original edited\n\n" } else { "Original edited" };
    assert_eq!(contents.as_string().unwrap().to_text(), expected, "only a typewriter edit preserves trailing blank lines");
    let mut reopened = pdfcraft_engine::Session::new();
    let again = reopened.open("edited.pdf", None, bytes, None).unwrap();
    let saved = reopened.get(again).unwrap().info.annotations.iter().find(|a| a.page == 1 && a.index == 0).unwrap();
    assert_eq!(saved.contents.as_deref(), Some(expected));
}

#[test]
fn typewriter_edit_composer_keeps_trailing_blank_lines() {
    edit_composer_preserves_only_typewriter_whitespace(0);
}

#[test]
fn other_edit_composers_keep_trimming_trailing_whitespace() {
    for kind in [1, 2] {
        edit_composer_preserves_only_typewriter_whitespace(kind);
    }
}

/// ⌘-click (Ctrl on Linux and Windows) adds comments to the selection and takes one out again;
/// Delete removes every selected comment as one undo step.
#[test]
fn ctrl_click_selects_several_comments_and_delete_removes_them_all() {
    let mut h = visible_page_harness(|app| app.set_option(&egui::Context::default(), "quick", "square").unwrap());
    for (a, b) in [((20.0, 20.0), (60.0, 60.0)), ((100.0, 20.0), (140.0, 60.0)), ((180.0, 20.0), (220.0, 60.0))] {
        drag_pt(&mut h, a, b);
    }
    assert_eq!(comments(&h).len(), 3);
    h.set_option("quick", "select").unwrap();
    h.run_steps(2);
    click_pt(&mut h, (40.0, 40.0));
    assert_eq!(h.state().views[0].session.comments.selection(), vec![(0, 0)]);
    click_pt_with(&mut h, (120.0, 40.0), egui::Modifiers::COMMAND);
    click_pt_with(&mut h, (200.0, 40.0), egui::Modifiers::COMMAND);
    assert_eq!(h.state().views[0].session.comments.selection(), vec![(0, 2), (0, 0), (0, 1)], "the handles go to the one clicked last");
    assert_eq!(h.state().views[0].session.comments.primary(), Some((0, 2)));
    // ⌘-click on a selected comment takes it out again.
    click_pt_with(&mut h, (40.0, 40.0), egui::Modifiers::COMMAND);
    assert_eq!(h.state().views[0].session.comments.selection(), vec![(0, 2), (0, 1)]);
    // Delete: both go, as one step.
    h.key_press(egui::Key::Delete);
    h.run_steps(3);
    let left = comments(&h);
    assert_eq!(left.len(), 1, "{left:?}");
    assert!((left[0].rect[0] - 20.0).abs() < 1.0, "the unselected first rectangle stays: {:?}", left[0].rect);
    assert!(h.state().views[0].session.comments.selection().is_empty());
    let id = h.state().views[0].id;
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Delete comments"));
    undo(&mut h);
    assert_eq!(comments(&h).len(), 3);
    // A plain click selects one alone again.
    click_pt(&mut h, (120.0, 40.0));
    assert_eq!(h.state().views[0].session.comments.selection(), vec![(0, 1)]);
    // On the page ⇧-click adds one too, and takes it out again (a range is the panel's).
    click_pt_with(&mut h, (200.0, 40.0), egui::Modifiers::SHIFT);
    assert_eq!(h.state().views[0].session.comments.selection().len(), 2, "{:?}", h.state().views[0].session.comments.selection());
    click_pt_with(&mut h, (200.0, 40.0), egui::Modifiers::SHIFT);
    assert_eq!(h.state().views[0].session.comments.selection(), vec![(0, 1)]);
}

/// Dragging one of several selected comments moves them all, whichever of them is grabbed: the
/// grabbed one takes the handles and the one that had them stays selected.
#[test]
fn dragging_any_selected_comment_moves_the_whole_selection() {
    let mut h = visible_page_harness(|app| app.set_option(&egui::Context::default(), "quick", "square").unwrap());
    for (a, b) in [((20.0, 20.0), (60.0, 60.0)), ((100.0, 20.0), (140.0, 60.0)), ((180.0, 20.0), (220.0, 60.0))] {
        drag_pt(&mut h, a, b);
    }
    h.set_option("quick", "select").unwrap();
    h.run_steps(2);
    click_pt(&mut h, (40.0, 40.0));
    click_pt_with(&mut h, (120.0, 40.0), egui::Modifiers::COMMAND);
    click_pt_with(&mut h, (200.0, 40.0), egui::Modifiers::COMMAND);
    assert_eq!(h.state().views[0].session.comments.primary(), Some((0, 2)), "the last one clicked has the handles");
    // Drag the first one (not the one with the handles) 30 points up.
    let before = comments(&h);
    drag_pt(&mut h, (40.0, 40.0), (40.0, 70.0));
    let after = comments(&h);
    assert_eq!(after.len(), 3);
    for (a, b) in before.iter().zip(&after) {
        assert!((b.rect[1] - a.rect[1] - 30.0).abs() < 1.5, "every selected rectangle moved up 30 pt: {:?} from {:?}", b.rect, a.rect);
    }
    let selection = h.state().views[0].session.comments.selection();
    assert_eq!(selection.len(), 3, "all three stay selected: {selection:?}");
    assert_eq!(selection[0], (0, 0), "the grabbed one has the handles");
    let id = h.state().views[0].id;
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Move comments"));
    // A locked one in the selection stays where it is; the others still move (it would refuse
    // the whole step otherwise, and nothing would move).
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().apply_edit(&ctx_h, pdfcraft_engine::Edit::LockAnnotation { page: 0, index: 1, locked: true }));
    h.run_steps(2);
    assert_eq!(h.state().views[0].session.comments.selection().len(), 3);
    let before = comments(&h);
    drag_pt(&mut h, (40.0, 70.0), (40.0, 90.0));
    let after = comments(&h);
    // By index: the list isn't kept in index order.
    let bottom = |list: &[Annotation], i: usize| list.iter().find(|a| a.index == i).map(|a| a.rect[1]).expect("still there");
    for i in 0..3 {
        let moved = bottom(&after, i) - bottom(&before, i);
        if i == 1 {
            assert!(moved.abs() < 0.01, "the locked one stays: moved {moved}");
        } else {
            assert!((moved - 20.0).abs() < 1.5, "rectangle {i} moved up 20 pt: moved {moved}");
        }
    }
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Move comments"));
}

/// Copied comments on the pointer take the click whatever tool is on: with Fill & Sign's Text
/// tool or a stamp tool still on, the click places the bundle and opens no text box or stamp.
#[test]
fn copied_comments_take_the_click_from_the_tool_that_is_on() {
    use pdfcraft_ui_egui::fill_sign::FillTool;
    let mut h = harness(|_| {});
    let id = h.state().views[0].id;
    let shape = pdfcraft_engine::Shape::Rectangle { rect: [20.0, 20.0, 60.0, 60.0] };
    let style = pdfcraft_engine::Style::default_for(&shape);
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().apply_edit(
        &ctx_h,
        pdfcraft_engine::Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
            page: 0,
            shape,
            style,
            contents: String::new(),
            author: "Tester".into(),
        })
    ));
    h.run_steps(2);
    h.state_mut().views[0].session.comments.select_only((0, 0));
    assert!(h.execute("comment.copy"));
    let count = |h: &Harness<'static, PdfCraftApp>| h.state().session.get(id).unwrap().info.annotations.len();
    for (tool, at) in [(QuickTool::Fill(FillTool::Text), (150.0, 100.0)), (QuickTool::Stamp(pdfcraft_engine::StampKind::Approved), (200.0, 160.0))] {
        h.state_mut().quick_tool = tool;
        h.run_steps(2);
        let before = count(&h);
        assert!(h.execute("comment.paste"));
        click_pt(&mut h, at);
        assert_eq!(count(&h), before + 1, "{tool:?}: the copy, and nothing else");
        assert!(h.state().views[0].session.fill_text.is_none(), "{tool:?}: no text box opens");
        assert!(h.state().views[0].session.comments.tool.in_flight().is_none());
        assert!(h.state().views[0].session.comments.paste.is_none(), "{tool:?}: the bundle is let go once placed");
        let placed = h.state().session.get(id).unwrap().info.annotations.iter().max_by_key(|a| a.index).map(|a| a.rect).expect("placed");
        assert!(((placed[0] + placed[2]) / 2.0 - at.0).abs() < 1.0, "{tool:?}: centred on the click: {placed:?}");
        assert_eq!(h.state().quick_tool, tool, "the tool stays on for the next click");
    }
    // Afterwards the tool takes its clicks again.
    h.state_mut().quick_tool = QuickTool::Fill(FillTool::Text);
    h.run_steps(2);
    click_pt(&mut h, (100.0, 200.0));
    assert!(h.state().views[0].session.fill_text.is_some(), "Fill & Sign's Text tool opens its box again");
}

/// Two boxes on page 1 of the active document, and a reply to the first.
fn add_two_boxes_and_a_reply(h: &mut Harness<'static, PdfCraftApp>) {
    for rect in [[20.0, 20.0, 60.0, 60.0], [100.0, 20.0, 140.0, 60.0]] {
        let shape = pdfcraft_engine::Shape::Rectangle { rect };
        let style = pdfcraft_engine::Style::default_for(&shape);
        let ctx_h = h.ctx.clone();
        assert!(h.state_mut().apply_edit(
            &ctx_h,
            pdfcraft_engine::Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
                page: 0,
                shape,
                style,
                contents: "Box".into(),
                author: "Tester".into(),
            })
        ));
    }
    let ctx_h = h.ctx.clone();
    assert!(
        h.state_mut()
            .apply_edit(&ctx_h, pdfcraft_engine::Edit::ReplyToAnnotation { page: 0, index: 0, text: "Reply".into(), author: "Tester".into() })
    );
    h.run_steps(2);
}

/// Copied comments paste into another document on its current page, each with its replies, as
/// new comments (a second paste gives a second set); the copies are selected afterwards.
#[test]
fn copied_comments_paste_into_another_document() {
    let mut h = harness(|app| {
        app.open_bytes(&egui::Context::default(), "other.pdf", None, TEXT_FIXTURE.to_vec()).expect("opens");
        app.active = Some(0);
    });
    let (source, target) = (h.state().views[0].id, h.state().views[1].id);
    add_two_boxes_and_a_reply(&mut h);
    // Adding a comment selects it; with nothing selected there is nothing to copy.
    h.state_mut().views[0].session.comments.clear_selection();
    assert!(!h.execute("comment.copy"), "nothing selected: nothing to copy");
    {
        let cv = &mut h.state_mut().views[0].session.comments;
        cv.select_only((0, 0));
        cv.toggle((0, 1));
    }
    assert!(h.execute("comment.copy"));
    assert_eq!(h.state().comment_clipboard.as_ref().map(|c| c.count), Some(2));
    // Into the other document: Edit ▸ Paste comments hangs the bundle on the pointer, a click
    // places it, centred there (the two boxes span 20..140 × 20..60: centre 80, 40).
    h.state_mut().active = Some(1);
    h.run_steps(2);
    assert!(h.execute("comment.paste"));
    let count = |h: &Harness<'static, PdfCraftApp>| h.state().session.get(target).unwrap().info.annotations.len();
    assert_eq!(count(&h), 0, "nothing placed yet");
    assert_eq!(h.state().views[1].session.comments.paste.as_ref().map(|p| p.count), Some(2));
    assert!(h.state().views[1].session.comments.tool.in_flight().is_none(), "the bundle is no move or resize");
    click_pt(&mut h, (150.0, 100.0));
    let pasted = h.state().session.get(target).unwrap().info.annotations.clone();
    assert_eq!(pasted.len(), 3, "two comments and the reply: {pasted:?}");
    assert!(pasted.iter().all(|a| a.page == 0), "on the page clicked: {pasted:?}");
    let parent = pasted.iter().find(|a| a.in_reply_to.is_none() && (a.rect[0] - 90.0).abs() < 1.0).expect("the first box, moved by (70, 60)");
    assert!((parent.rect[1] - 80.0).abs() < 1.0 && (parent.rect[2] - 130.0).abs() < 1.0, "{:?}", parent.rect);
    assert_eq!(pasted.iter().find(|a| a.in_reply_to.is_some()).and_then(|r| r.in_reply_to.clone()), parent.name, "the reply follows its copy");
    assert_eq!(h.state().views[1].session.comments.selection().len(), 2, "the copies are selected");
    assert!(h.state().views[1].session.comments.paste.is_none());
    assert_eq!(h.state().session.get(target).unwrap().can_undo(), Some("Paste comments"));
    assert_eq!(h.state().session.get(source).unwrap().info.annotations.len(), 3, "the source is untouched");
    // The page menu's Paste comments places the bundle where the menu was opened: page 2 here.
    h.state_mut().views[1].session.comments.paste_request = Some((1, [100.0, 100.0]));
    h.run_steps(3);
    assert_eq!(count(&h), 6);
    let second: Vec<_> =
        h.state().session.get(target).unwrap().info.annotations.iter().filter(|a| a.page == 1 && a.in_reply_to.is_none()).cloned().collect();
    assert_eq!(second.len(), 2);
    let centre_x = (second.iter().map(|a| a.rect[0]).fold(f32::MAX, f32::min) + second.iter().map(|a| a.rect[2]).fold(f32::MIN, f32::max)) / 2.0;
    assert!((centre_x - 100.0).abs() < 1.0, "centred at the menu's point: {second:?}");
    // ⌘V (a Paste event) hangs the bundle on the pointer again; Escape lets it go.
    h.event(egui::Event::Paste(String::new()));
    h.run_steps(3);
    assert!(h.state().views[1].session.comments.paste.is_some());
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(h.state().views[1].session.comments.paste.is_none());
    assert_eq!(count(&h), 6, "nothing placed");
    undo(&mut h);
    assert_eq!(count(&h), 3);
    // XFDF arriving from the system clipboard pastes too, even without the app's clipboard.
    let xfdf = h.state().comment_clipboard.clone().unwrap().xfdf;
    h.state_mut().comment_clipboard = None;
    assert!(!h.execute("comment.paste"), "nothing copied: disabled");
    h.event(egui::Event::Paste(xfdf.clone()));
    h.run_steps(3);
    click_pt(&mut h, (200.0, 120.0));
    assert_eq!(count(&h), 6);
    h.event(egui::Event::Paste("just text".into()));
    h.run_steps(3);
    assert!(h.state().views[1].session.comments.paste.is_none(), "plain text is not a paste of comments");
    // Nor when comments were copied earlier: a word copied in another app since is that word.
    h.state_mut().comment_clipboard = Some(pdfcraft_ui_egui::CommentClip { xfdf, count: 2 });
    h.event(egui::Event::Paste("a word from elsewhere".into()));
    h.run_steps(3);
    assert!(h.state().views[1].session.comments.paste.is_none(), "the old comments don't hang on the pointer");
    // Nothing legible on the system clipboard: the app's own copied comments.
    h.event(egui::Event::Paste(String::new()));
    h.run_steps(3);
    assert!(h.state().views[1].session.comments.paste.is_some());
}

/// Connected lines (and polygons) finish from the right-click menu too, as other editors do;
/// the menu also takes the last point back and cancels the drawing.
#[test]
fn connected_lines_are_completed_from_the_context_menu() {
    let mut h = visible_page_harness(|app| app.set_option(&egui::Context::default(), "quick", "polyline").unwrap());
    for p in [(20.0, 150.0), (60.0, 130.0), (100.0, 150.0)] {
        click_pt(&mut h, p);
    }
    assert_eq!(comments(&h).len(), 0, "still drawing");
    let right_click = |h: &mut Harness<'static, PdfCraftApp>, p: (f32, f32)| {
        let p = at(h, p.0, p.1);
        h.hover_at(p);
        h.run_steps(1);
        for pressed in [true, false] {
            h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed, modifiers: egui::Modifiers::NONE });
            h.run_steps(1);
        }
        h.run_steps(2);
    };
    right_click(&mut h, (120.0, 100.0));
    h.get_by_label("Complete").click();
    h.run_steps(3);
    let c = comments(&h);
    assert_eq!(c.len(), 1, "{c:?}");
    assert_eq!(c[0].subtype, "PolyLine");
    // A polygon needs three points: with two, Complete is greyed; Undo last point and Cancel drawing work.
    h.set_option("quick", "polygon").unwrap();
    for p in [(150.0, 150.0), (200.0, 150.0)] {
        click_pt(&mut h, p);
    }
    assert_eq!(drawn_points(&h), 2);
    right_click(&mut h, (220.0, 100.0));
    assert!(h.get_by_label("Complete").accesskit_node().is_disabled());
    h.get_by_label("Undo last point").click();
    h.run_steps(3);
    assert_eq!(drawn_points(&h), 1);
    right_click(&mut h, (220.0, 100.0));
    h.get_by_label("Cancel drawing").click();
    h.run_steps(3);
    assert_eq!(drawn_points(&h), 0);
    assert!(h.state().views[0].session.comments.tool.drawing().is_empty());
    assert_eq!(comments(&h).len(), 1, "nothing else was drawn");
    // On the second page, with the first on screen in the same frame: Complete reaches the page
    // the drawing is on (the first page passes first and must not take the request).
    h.set_option("quick", "polyline").unwrap();
    h.set_option("zoom", "50").unwrap();
    crate::support::settle(&mut h);
    assert!(h.state().views[0].page_screen_rect(0).is_some(), "page 1 is on screen too");
    let at2 = |h: &Harness<'static, PdfCraftApp>, x: f32, y: f32| {
        let r = h.state().views[0].page_screen_rect(1).expect("page 2 on screen");
        pos2(r.left() + x / 300.0 * r.width(), r.top() + (200.0 - y) / 200.0 * r.height())
    };
    for (x, y) in [(20.0, 100.0), (60.0, 80.0)] {
        let p = at2(&h, x, y);
        click(&mut h, p);
    }
    assert_eq!(drawn_points(&h), 2);
    let p = at2(&h, 150.0, 150.0);
    h.hover_at(p);
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed, modifiers: egui::Modifiers::NONE });
        h.run_steps(1);
    }
    h.run_steps(2);
    h.get_by_label("Complete").click();
    h.run_steps(3);
    let c = comments(&h);
    assert_eq!(c.len(), 2, "{c:?}");
    assert!(c.iter().any(|a| a.page == 1 && a.subtype == "PolyLine"), "{c:?}");
    assert!(h.state().views[0].session.comments.tool.drawing().is_empty());
}

/// Properties on one of several selected comments changes all of them, each where it applies: the
/// colour goes to every one, the line width to the shapes, the font size to the text box alone;
/// a locked one stays as it is. One undo step. Colouring the selection works the same way.
#[test]
fn properties_change_every_selected_comment_where_it_applies() {
    use pdfcraft_engine::{Edit, NewAnnotation, Shape, Style};
    let mut h = harness(|_| {});
    let shapes = [
        Shape::Rectangle { rect: [20.0, 20.0, 60.0, 60.0] },
        Shape::Oval { rect: [80.0, 20.0, 120.0, 60.0] },
        Shape::TextBox { rect: [140.0, 20.0, 200.0, 40.0], font_size: 12.0 },
        Shape::Rectangle { rect: [220.0, 20.0, 260.0, 60.0] },
    ];
    for shape in shapes {
        let style = Style::default_for(&shape);
        let ctx_h = h.ctx.clone();
        assert!(
            h.state_mut()
                .apply_edit(&ctx_h, Edit::AddAnnotation(NewAnnotation { page: 0, shape, style, contents: "x".into(), author: "Tester".into() }))
        );
    }
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().apply_edit(&ctx_h, Edit::LockAnnotation { page: 0, index: 3, locked: true }));
    {
        let c = &mut h.state_mut().views[0].session.comments;
        c.select_only((0, 0));
        for key in [(0, 1), (0, 2), (0, 3)] {
            c.toggle(key);
        }
    }
    h.run_steps(2);
    open_comment_props(&mut h, 0, 0);
    h.run_steps(2);
    // The dialog says the changes go to every selected comment.
    h.get_by(|node| {
        node.role() == Role::Heading
            && node.description().is_some_and(|description| description.contains("Changes here apply to all 4 selected comments"))
    });
    set_prop(&mut h, PROP_COLOUR, FieldValue::Color([0.0, 0.0, 1.0]));
    set_prop(&mut h, "width", FieldValue::Number(4.0));
    accept_props(&mut h);
    let props = |h: &Harness<'static, PdfCraftApp>, index: usize| {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().comment_props(0, index).unwrap()
    };
    for index in [0, 1] {
        let p = props(&h, index);
        assert_eq!((p.color, p.width), (Some([0.0, 0.0, 1.0]), Some(4.0)), "shape {index}");
    }
    assert_eq!(props(&h, 2).color, Some([0.0, 0.0, 1.0]), "the text box's text colour");
    let locked = props(&h, 3);
    assert_ne!(locked.color, Some([0.0, 0.0, 1.0]), "the locked rectangle stays as it was");
    let s = h.state();
    assert_eq!(s.session.get(s.views[0].id).unwrap().can_undo(), Some("Change comment properties"), "one undo step");
    // From the text box: its font size changes, and the shapes, which have none, keep their look.
    h.state_mut().views[0].session.comments.select_only((0, 2));
    for key in [(0, 0), (0, 1)] {
        h.state_mut().views[0].session.comments.toggle(key);
    }
    open_comment_props(&mut h, 0, 2);
    h.run_steps(2);
    set_prop(&mut h, "font_size", FieldValue::Number(20.0));
    accept_props(&mut h);
    assert_eq!(props(&h, 2).font_size, Some(20.0));
    assert_eq!((props(&h, 0).font_size, props(&h, 0).width), (None, Some(4.0)));
    // Colouring the selection (the context menu's Colour with several selected).
    let ctx_h = h.ctx.clone();
    h.state_mut().colour_selected_comments(&ctx_h, [1.0, 0.0, 0.0]);
    h.run_steps(2);
    for index in [0, 1, 2] {
        assert_eq!(props(&h, index).color, Some([1.0, 0.0, 0.0]), "comment {index}");
    }
    assert_ne!(props(&h, 3).color, Some([1.0, 0.0, 0.0]));
}

/// Comment Properties' text opacity, set with several comments selected, goes to the other text
/// boxes whose text shows an opacity of its own (a border or a fill), and leaves a text-only box,
/// whose one opacity is the whole comment's, as it was.
#[test]
fn part_opacity_changes_every_selected_comment_that_shows_the_part() {
    use pdfcraft_engine::{Edit, NewAnnotation, Shape, Style};
    let mut h = harness(|_| {});
    for (n, width) in [1.0, 1.0, 0.0].into_iter().enumerate() {
        let x = 20.0 + 100.0 * n as f64;
        let shape = Shape::TextBox { rect: [x, 20.0, x + 80.0, 40.0], font_size: 12.0 };
        let style = Style { width, ..Style::default_for(&shape) };
        let ctx_h = h.ctx.clone();
        assert!(
            h.state_mut()
                .apply_edit(&ctx_h, Edit::AddAnnotation(NewAnnotation { page: 0, shape, style, contents: "x".into(), author: "Tester".into() }))
        );
    }
    let props = |h: &Harness<'static, PdfCraftApp>, index: usize| {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().comment_props(0, index).unwrap()
    };
    let text_only = props(&h, 2);
    {
        let c = &mut h.state_mut().views[0].session.comments;
        c.select_only((0, 0));
        for key in [(0, 1), (0, 2)] {
            c.toggle(key);
        }
    }
    h.run_steps(2);
    open_comment_props(&mut h, 0, 0);
    h.run_steps(2);
    set_prop(&mut h, "text_opacity", FieldValue::Number(40.0));
    accept_props(&mut h);
    for index in [0, 1] {
        let text = props(&h, index).text_opacity.expect("a text box's text opacity");
        assert!((text - 0.4).abs() < 1e-3, "bordered box {index}: {text}");
    }
    let after = props(&h, 2);
    assert_eq!((after.text_opacity, after.opacity), (text_only.text_opacity, text_only.opacity), "the text-only box keeps its look");
}

/// Escape with a Fill & Sign tool on and several comments selected deselects them all first,
/// then drops the tool.
#[test]
fn escape_with_a_fill_tool_deselects_every_comment_then_drops_the_tool() {
    use pdfcraft_ui_egui::fill_sign::FillTool;
    let mut h = harness(|_| {});
    add_two_boxes_and_a_reply(&mut h);
    h.state_mut().quick_tool = QuickTool::Fill(FillTool::Text);
    {
        let c = &mut h.state_mut().views[0].session.comments;
        c.select_only((0, 0));
        c.toggle((0, 1));
    }
    h.run_steps(2);
    assert_eq!(h.state().views[0].session.comments.selection().len(), 2);
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(h.state().views[0].session.comments.selection().is_empty(), "the whole selection goes");
    assert_eq!(h.state().quick_tool, QuickTool::Fill(FillTool::Text), "the tool stays for now");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert_eq!(h.state().quick_tool, QuickTool::Select);
}

/// A square drawn and selected with the Select tool.
fn selected_square() -> Harness<'static, PdfCraftApp> {
    let mut h = harness(|app| app.set_option(&egui::Context::default(), "quick", "square").unwrap());
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    h.set_option("quick", "select").unwrap();
    click_pt(&mut h, (250.0, 20.0));
    click_pt(&mut h, (90.0, 70.0));
    assert_eq!(h.state().views[0].session.comments.selection(), vec![(0, 0)]);
    h
}

/// How the Edit ▸ Delete row of `edit_delete_removes_the_selected_comment_with_or_without_a_popup`
/// starts.
#[derive(Clone, Copy, Debug)]
enum DeleteWith {
    /// Nothing open over the comment.
    Nothing,
    /// The comment's own page menu is open: the native menu's click never reaches the window, so
    /// Edit ▸ Delete closes the popup, as a click anywhere else would, before its key joins the
    /// pages'.
    PageMenu,
    /// The comment's pop-up window is open; it goes with the comment.
    PopupWindow,
}

/// Edit ▸ Delete removes the selected comment (the key a menu click stands for is not under the
/// menu it came from), with nothing open, with the comment's page menu open, and with its pop-up
/// window open, which goes with it.
#[test]
fn edit_delete_removes_the_selected_comment_with_or_without_a_popup() {
    for row in [DeleteWith::Nothing, DeleteWith::PageMenu, DeleteWith::PopupWindow] {
        let mut h = match row {
            DeleteWith::PopupWindow => {
                let mut h = harness_with_bytes(BLUE_NOTE_WITH_OPEN_POP_UP.to_vec(), |_| {});
                assert_eq!(popup_windows(&h, "Alpha").len(), 1, "{row:?}: the pop-up is open");
                h.state_mut().views[0].session.comments.select_only((0, 0));
                h.run_steps(2);
                h
            }
            _ => selected_square(),
        };
        if matches!(row, DeleteWith::PageMenu) {
            right_click_pt(&mut h, (90.0, 70.0));
            assert!(h.query_by_label("Delete").is_some(), "{row:?}: the comment's menu is open");
        }
        support::fire_menu(&mut h, "edit.delete");
        h.run_steps(4);
        assert!(comments(&h).is_empty(), "{row:?}: Edit ▸ Delete removed the comment");
        assert!(popup_windows(&h, "Alpha").is_empty(), "{row:?}: no pop-up window is left");
    }
}

/// Full screen hides the rail and its drawer, which don't come back with the rail: back from full
/// screen, a Delete on the pass that shows the rail again removes the selected comment with
/// nothing over it. The drawer came back, drawn after that pass's keys.
#[test]
fn full_screen_closes_the_hotbar_tools_window() {
    let mut h = selected_square();
    h.get_by_label("Edit toolbar").click();
    h.run_steps(3);
    assert!(drawer_search(&h).is_some(), "the rail's drawer is open");
    support::fire_menu(&mut h, "view.full_screen");
    h.run_steps(4);
    assert!(h.state().full_screen(), "full screen");
    assert!(h.query_by_label("Edit toolbar").is_none() && drawer_search(&h).is_none(), "the rail and its drawer are hidden");
    h.run_steps(3);
    assert!(drawer_search(&h).is_none(), "and stay hidden");
    // Back, as a native menu's click is, between passes.
    support::fire_menu(&mut h, "view.full_screen");
    h.key_press(egui::Key::Delete);
    h.run_steps(4);
    assert!(!h.state().full_screen(), "back from full screen");
    assert!(drawer_search(&h).is_none(), "the drawer didn't come back");
    assert!(comments(&h).is_empty(), "Delete removed the comment");
}

/// "Open in Full Screen mode?", asked for a document that wants it, leaves the menus on as the
/// update prompt does, and covers the pages as it does, in full screen too: neither the user's
/// Delete nor Edit ▸ Delete from a native menu removes the comment selected in another document
/// under it, and the Escape that answers the prompt is the prompt's alone.
#[test]
fn edit_delete_from_a_native_menu_under_the_full_screen_prompt_leaves_the_comment() {
    for full_screen in [false, true] {
        let mut h = selected_square();
        if full_screen {
            assert!(h.execute("view.full_screen"));
            h.run_steps(4);
            assert!(h.state().full_screen(), "full screen");
        }
        let deck = String::from_utf8(TEXT_FIXTURE.to_vec()).unwrap().replace("/Type /Catalog", "/Type /Catalog /PageMode /FullScreen").into_bytes();
        let ctx_h = h.ctx.clone();
        h.state_mut().open_bytes(&ctx_h, "deck.pdf", None, deck).expect("the deck opens");
        h.run_steps(3);
        let prompt = |h: &Harness<'static, PdfCraftApp>| h.query_by_role_and_label(Role::Dialog, "Open in Full Screen mode?").is_some();
        assert!(prompt(&h), "the full-screen prompt is open");
        support::fire_menu(&mut h, craft::menu::CommandRef::with_params("window.document", serde_json::json!({ "index": 0 })));
        h.run_steps(3);
        assert_eq!(h.state().active, Some(0), "Window ▸ the first document switched to it");
        assert!(prompt(&h), "the full-screen prompt is still open");
        assert_eq!(h.state().views[0].session.comments.selection(), vec![(0, 0)], "the comment is still selected");
        h.key_press(egui::Key::Delete);
        h.run_steps(3);
        assert_eq!(comments(&h).len(), 1, "Delete under the full-screen prompt deleted the comment (full screen: {full_screen})");
        support::fire_menu(&mut h, "edit.delete");
        h.run_steps(4);
        assert_eq!(comments(&h).len(), 1, "Edit ▸ Delete under the full-screen prompt deleted the comment (full screen: {full_screen})");
        h.key_press(egui::Key::Escape);
        h.run_steps(3);
        assert!(!prompt(&h), "Escape answered the prompt");
        assert_eq!(h.state().full_screen(), full_screen, "the Escape that answered the prompt is the prompt's alone");
    }
}

/// #5767: Make Properties Default on a text box or callout keeps its font size too, so the next
/// one (also after a restart) takes it, along with the colour.
#[test]
fn properties_dialog_makes_the_text_box_font_size_default() {
    let blue = [0.0, 0.47, 0.84];
    let mut h = harness(|_| {});
    let font_size_of_last = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let last = doc.info.annotations.len() - 1;
        doc.comment_props(0, last).unwrap()
    };
    for (quick, size, first, second, third) in
        [("freetext", 24.0, (40.0, 60.0), (40.0, 120.0), (40.0, 100.0)), ("callout", 18.0, (150.0, 40.0), (150.0, 100.0), (150.0, 60.0))]
    {
        h.set_option("quick", quick).unwrap();
        if quick == "callout" {
            drag_pt(&mut h, first, (first.0 + 60.0, first.1 + 20.0));
        } else {
            click_pt(&mut h, first);
        }
        post_text(&mut h, "First");
        let index = comments(&h).len() - 1;
        open_comment_props(&mut h, 0, index);
        h.run_steps(2);
        set_prop(&mut h, "font_size", FieldValue::Number(size));
        set_prop(&mut h, PROP_COLOUR, FieldValue::Color(blue.map(|c| c as f32)));
        set_prop(&mut h, "make_default", FieldValue::Bool(true));
        accept_props(&mut h);
        assert_eq!(font_size_of_last(&h).font_size, Some(size));

        // The next one takes the font size and the colour.
        h.set_option("quick", quick).unwrap();
        if quick == "callout" {
            drag_pt(&mut h, second, (second.0 + 60.0, second.1 + 20.0));
        } else {
            click_pt(&mut h, second);
        }
        post_text(&mut h, "Second");
        let p = font_size_of_last(&h);
        assert_eq!(p.font_size, Some(size), "{quick}: the next one takes the default font size");
        assert_eq!(p.color, Some(blue), "{quick}: and the default colour");

        // The default survives a restart.
        let saved = h.state().settings_snapshot();
        let mut fresh = harness(move |app| {
            app.apply_settings(&saved);
            app.set_option(&egui::Context::default(), "quick", quick).unwrap();
        });
        if quick == "callout" {
            drag_pt(&mut fresh, third, (third.0 + 60.0, third.1 + 20.0));
        } else {
            click_pt(&mut fresh, third);
        }
        post_text(&mut fresh, "Third");
        assert_eq!(font_size_of_last(&fresh).font_size, Some(size), "{quick}: the default font size persisted");
    }
}

#[test]
fn ctrl_selected_lines_nudge_together_and_undo() {
    let mut h = harness(|app| app.set_option(&egui::Context::default(), "quick", "line").unwrap());
    for y in [40.0, 80.0, 120.0] {
        drag_pt(&mut h, (40.0, y), (160.0, y));
    }
    assert_eq!(comments(&h).len(), 3);
    h.set_option("quick", "select").unwrap();
    h.run_steps(2);
    click_pt(&mut h, (100.0, 40.0));
    click_pt_with(&mut h, (100.0, 80.0), egui::Modifiers::COMMAND);
    click_pt_with(&mut h, (100.0, 120.0), egui::Modifiers::COMMAND);
    assert_eq!(h.state().views[0].session.comments.selection().len(), 3);
    let before: Vec<_> = comments(&h).iter().map(|a| a.rect).collect();
    h.key_press(egui::Key::ArrowRight);
    h.run_steps(3);
    let moved: Vec<_> = comments(&h).iter().map(|a| a.rect).collect();
    for (old, new) in before.iter().zip(&moved) {
        assert!(new[0] > old[0], "each selected line moves: {before:?} -> {moved:?}");
    }
    undo(&mut h);
    h.run_steps(2);
    assert_eq!(comments(&h).iter().map(|a| a.rect).collect::<Vec<_>>(), before);
}

/// Row of `panel_and_card_actions_become_engine_edits`: a card's "Set status" gives the engine a
/// status reply of the chosen state.
fn set_status_row() {
    let mut h = harness(|app| {
        app.set_option(&egui::Context::default(), "panel", "comments").unwrap();
        app.set_option(&egui::Context::default(), "quick", "square").unwrap();
    });
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    h.set_option("quick", "select").unwrap();
    h.state_mut().views[0].session.comments.select_only((0, 0));
    h.run_steps(2);
    h.get_by_label("More").click();
    h.run_steps(2);
    h.get_by_label("Set status").click();
    h.run_steps(2);
    h.get_by_label("Accepted").click();
    h.run_steps(3);
    assert!(comments(&h).iter().any(|a| a.state.as_deref() == Some("Accepted")), "the card's status reached the engine");
}

/// What the Comments panel's cards and menus give the engine: an `Add` and a `Reply` from the
/// boxes, a `SetStatus` from the card menu, a `Delete` of one reply (with its locked gates), the
/// attachment's open request and the panel's Flatten comments. Each row drives the framework
/// panel by label on its own document.
#[test]
fn panel_and_card_actions_become_engine_edits() {
    let rows: [(&str, fn()); 5] = [
        ("add a comment and a reply", panel_posts_comment_and_reply_row),
        ("set a status", set_status_row),
        ("delete one reply", delete_one_reply_row),
        ("open an attachment", attachment_card_menu_row),
        ("flatten from the panel menu", flatten_from_the_panel_menu_row),
    ];
    for (_name, row) in rows {
        row();
    }
}

/// A one-page PDF with one yellow highlight at (20, 140)–(120, 165), encrypted so that only the
/// `permissions` bits are allowed.
fn encrypted_highlight(permissions: i32) -> Vec<u8> {
    let plain = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Annots [4 0 R] >> endobj
4 0 obj << /Type /Annot /Subtype /Highlight /Rect [20 140 120 165] /QuadPoints [20 165 120 165 20 140 120 140] /C [1 0.94 0] /T (Ada) /Contents (Look here) >> endobj
trailer << /Root 1 0 R >>
%%EOF";
    let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(plain.to_vec())).expect("fixture parses");
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: "",
        owner_password: "owner",
        permissions,
        encrypt_metadata: true,
        seed: [6; 32],
    })
    .expect("encrypts");
    pdfcraft_cos::write_full(&doc, &Default::default()).expect("writes")
}

/// What a document allows in a row of `comment_menu_rows_follow_permissions_and_run_their_action`.
#[derive(Clone, Copy, Debug)]
enum Rights {
    /// Not encrypted: everything.
    Everything,
    /// Commenting is allowed, changing the document is not.
    NoChanges,
    /// Commenting and changing are both forbidden (printing only).
    NoComments,
}

/// Where the page menu of a row is opened.
#[derive(Clone, Copy, Debug)]
enum MenuAt {
    /// On the highlight, which the click selects first.
    Comment,
    /// On the page, away from any comment.
    Page,
}

/// What the row does when it is clicked.
#[derive(Clone, Copy, Debug)]
enum Does {
    Marks,
    OpensProperties,
    Deletes,
    Flattens,
    /// The row is greyed out and the document is left as it was.
    Disabled,
}

/// The page menu's rows follow what the document allows, and each enabled row runs its action:
/// checkmark, properties, delete and flatten for a document with every right; delete still works
/// where changes are forbidden but flatten does not; with comments forbidden the comment rows are
/// greyed out. The attachment's "Open in a new tab" is the last row.
#[test]
fn comment_menu_rows_follow_permissions_and_run_their_action() {
    let rows = [
        (Rights::Everything, MenuAt::Comment, "Mark with checkmark", Does::Marks),
        (Rights::Everything, MenuAt::Comment, "Properties…", Does::OpensProperties),
        (Rights::Everything, MenuAt::Comment, "Delete", Does::Deletes),
        (Rights::Everything, MenuAt::Page, "Flatten comments", Does::Flattens),
        (Rights::NoChanges, MenuAt::Comment, "Delete", Does::Deletes),
        (Rights::NoChanges, MenuAt::Page, "Flatten comments", Does::Disabled),
        (Rights::NoComments, MenuAt::Comment, "Delete", Does::Disabled),
        (Rights::NoComments, MenuAt::Comment, "Mark with checkmark", Does::Disabled),
        (Rights::NoComments, MenuAt::Comment, "Properties…", Does::Disabled),
    ];
    for (rights, at, label, does) in rows {
        let bytes = match rights {
            // Everything but bit 4 (modifying the document).
            Rights::NoChanges => encrypted_highlight(-1 ^ 8),
            // Printing only.
            Rights::NoComments => encrypted_highlight(0b0100),
            Rights::Everything => encrypted_highlight(-1),
        };
        let mut h = harness_with_bytes(bytes, |app| app.set_option(&egui::Context::default(), "quick", "select").unwrap());
        let what = format!("{rights:?}, {at:?}, {label}");
        assert_eq!(comments(&h).len(), 1, "{what}: the highlight is there");
        let point = match at {
            MenuAt::Comment => (70.0, 152.0),
            MenuAt::Page => (250.0, 20.0),
        };
        if matches!(at, MenuAt::Comment) {
            click_pt(&mut h, point);
            assert_eq!(h.state().views[0].session.comments.selection(), vec![(0, 0)], "{what}: the click selected the comment");
        }
        right_click_pt(&mut h, point);
        let disabled = h.get_by_role_and_label(Role::MenuItem, label).accesskit_node().is_disabled();
        assert_eq!(disabled, matches!(does, Does::Disabled), "{what}: the row is greyed out only where the rights forbid it");
        if disabled {
            h.key_press(egui::Key::Escape);
            h.run_steps(2);
            let id = h.state().views[0].id;
            assert_eq!(comments(&h).len(), 1, "{what}: nothing changed");
            assert_eq!(h.state().session.get(id).unwrap().can_undo(), None, "{what}: no edit was made");
            continue;
        }
        h.get_by_role_and_label(Role::MenuItem, label).click();
        h.run_steps(4);
        match does {
            Does::Marks => assert!(comments(&h).iter().any(|a| a.state.as_deref() == Some("Marked")), "{what}"),
            Does::OpensProperties => {
                assert!(h.state().comment_props.is_some(), "{what}: the hosted Comment Properties dialog is open");
            }
            Does::Deletes | Does::Flattens => assert!(comments(&h).is_empty(), "{what}: the comment is gone"),
            Does::Disabled => {}
        }
    }
    attachment_page_menu_row();
}
