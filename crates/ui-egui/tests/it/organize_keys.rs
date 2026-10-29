//! Organize Pages: the arrow keys move through the grid and ⇧ extends the selection with them;
//! a click where there is no page drops the selection (#3436).

use egui::{Key, Modifiers, PointerButton};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::Session;
use pdfcraft_ui_egui::PdfCraftApp;

const PAGES: usize = 20;

fn organize() -> Harness<'static, PdfCraftApp> {
    organize_pages(PAGES)
}

fn organize_pages(pages: usize) -> Harness<'static, PdfCraftApp> {
    let bytes = Session::new().create_blank(200.0, 300.0, pages).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("pages.pdf", None, bytes.as_ref().clone()).unwrap();
        app.set_option("organize", "on").unwrap();
        app
    });
    h.run_steps(4);
    h
}

fn state(h: &Harness<'static, PdfCraftApp>) -> (usize, Vec<usize>) {
    let view = &h.state().views[0];
    (view.current, view.selected.iter().copied().collect())
}

/// Pages in the grid's first row: the pages drawn level with page 1.
fn columns(h: &Harness<'static, PdfCraftApp>) -> usize {
    let top = h.get_by_label("Page 1").rect().top();
    (1..=PAGES).take_while(|p| h.get_by_label(&format!("Page {p}")).rect().top() == top).count()
}

fn press(h: &mut Harness<'static, PdfCraftApp>, modifiers: Modifiers, key: Key) {
    h.key_press_modifiers(modifiers, key);
    h.run_steps(2);
}

#[test]
fn arrow_keys_move_through_the_organize_grid_and_shift_extends_the_selection() {
    let mut h = organize();
    let cols = columns(&h);
    assert!((2..PAGES / 2).contains(&cols), "the test needs several rows of several pages: {cols}");
    assert_eq!(state(&h), (0, vec![]));
    press(&mut h, Modifiers::NONE, Key::ArrowLeft);
    assert_eq!(state(&h), (0, vec![]), "nothing before the first page");
    press(&mut h, Modifiers::NONE, Key::ArrowRight);
    assert_eq!(state(&h), (1, vec![]));
    press(&mut h, Modifiers::SHIFT, Key::ArrowRight);
    press(&mut h, Modifiers::SHIFT, Key::ArrowRight);
    assert_eq!(state(&h), (3, vec![1, 2, 3]), "⇧→ runs the selection from the page it started on");
    press(&mut h, Modifiers::SHIFT, Key::ArrowLeft);
    assert_eq!(state(&h), (2, vec![1, 2]), "and ⇧← shortens it again");
    press(&mut h, Modifiers::SHIFT, Key::ArrowDown);
    assert_eq!(state(&h), (2 + cols, (1..=2 + cols).collect()), "⇧↓ takes in a whole row");
    press(&mut h, Modifiers::NONE, Key::ArrowUp);
    assert_eq!(state(&h), (2, vec![]), "a plain arrow selects only the page it lands on");
    press(&mut h, Modifiers::NONE, Key::ArrowUp);
    assert_eq!(state(&h), (2, vec![]), "nothing above the first row");

    // The last row is short: ↓ from the row above it lands on the last page; from the last row
    // there is nowhere to go.
    let last_row = (PAGES - 1) / cols * cols;
    let above = last_row - 1;
    h.state_mut().views[0].current = above;
    press(&mut h, Modifiers::NONE, Key::ArrowDown);
    assert_eq!(state(&h).0, (above + cols).min(PAGES - 1));
    let landed = state(&h).0;
    press(&mut h, Modifiers::NONE, Key::ArrowDown);
    assert_eq!(state(&h).0, landed, "nothing below the last row");
}

#[test]
fn clicking_between_pages_in_the_organize_grid_drops_the_selection() {
    let mut h = organize();
    h.state_mut().views[0].select_pages(&[1, 2, 3]);
    h.run_steps(2);
    assert_eq!(state(&h).1, vec![1, 2, 3]);
    // Left of the first page: the grid's margin, where there is no page.
    let page = h.get_by_label("Page 1").rect();
    let empty = egui::pos2(page.left() - 8.0, page.center().y);
    h.hover_at(empty);
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: empty, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.run_steps(1);
    }
    h.run_steps(1);
    assert_eq!(state(&h).1, Vec::<usize>::new());
    assert_eq!(state(&h).0, 1, "the current page stays where it was");
}

/// Shift comes from each arrow press, not from the frame's modifier state: Shift pressed or let
/// go within one frame must not flip what an arrow does (#4993 review).
#[test]
fn each_arrow_press_uses_the_shift_it_was_pressed_with() {
    let mut h = organize();
    let key = |key: Key, modifiers: Modifiers| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
    // The frame's modifiers stay NONE; only the events carry Shift.
    h.event(key(Key::ArrowRight, Modifiers::SHIFT));
    h.event(key(Key::ArrowRight, Modifiers::SHIFT));
    h.run_steps(2);
    assert_eq!(state(&h), (2, vec![0, 1, 2]), "both presses in one frame extend the selection");
    h.event(egui::Event::ModifiersChanged(Modifiers::SHIFT));
    h.event(key(Key::ArrowLeft, Modifiers::NONE));
    h.run_steps(2);
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(1);
    assert_eq!(state(&h), (1, vec![]), "a plain press with Shift held later in the frame selects only its page");
}

/// An arrow press still brings its page into view when the grid zooms in the same frame or the
/// one before: the zoom keeps a page in view where it was, and that must not replace the
/// arrow's target (#4993 review). A guard only: the harness runs a second pass that settles the
/// zoom anchor before the arrow is read, so this passes without the fix as well.
#[test]
fn an_arrow_press_next_to_a_zoom_still_shows_its_page() {
    const MANY: usize = 60;
    let label = format!("Page {MANY}");
    for same_frame in [true, false] {
        let mut h = organize_pages(MANY);
        assert!(h.query_by_label(&label).is_none(), "the last page starts out of view");
        h.state_mut().views[0].current = MANY - 2;
        let z0 = h.state().views[0].grid_zoom();
        h.key_press_modifiers(Modifiers::COMMAND, Key::Minus);
        if !same_frame {
            h.run_steps(1);
        }
        h.key_press(Key::ArrowRight);
        h.run_steps(4);
        assert_eq!(state(&h).0, MANY - 1);
        eprintln!("DBG same={same_frame} zoom {z0} -> {} page1 {:?}", h.state().views[0].grid_zoom(), h.query_by_label("Page 1").map(|n| n.rect()));
        let page = h.query_by_label(&label).map(|n| n.rect());
        assert!(page.is_some_and(|r| r.top() >= 0.0 && r.bottom() <= 900.0), "same frame {same_frame}: the last page is in view: {page:?}");
    }
}
