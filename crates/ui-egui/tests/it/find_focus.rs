//! Find (⌘F) gives the find field the keyboard with the query selected, so typing replaces it,
//! also when the field already has the keyboard, and asking again while the field is composing
//! with an IME leaves the composition alone (the find bar and the Search panel).

use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.run_inline = true;
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
        app.set_option("left", "closed").unwrap();
        app
    });
    h.run_steps(4);
    h
}

fn query(h: &Harness<'static, PdfCraftApp>) -> String {
    h.state().views[0].find.as_ref().unwrap().query.clone()
}

#[test]
fn find_selects_the_query_when_the_field_takes_the_keyboard() {
    let mut h = harness();
    h.state_mut().views[0].open_find();
    h.run_steps(2);
    h.event(egui::Event::Text("cat".into()));
    h.run_steps(2);
    let id = h.state().views[0].find_input_id();
    h.ctx.memory_mut(|m| m.surrender_focus(id));
    h.run_steps(2);

    h.state_mut().views[0].open_find();
    h.run_steps(2);
    assert!(h.ctx.memory(|m| m.has_focus(id)));
    h.event(egui::Event::Text("dog".into()));
    h.run_steps(2);
    assert_eq!(query(&h), "dog", "typing replaces the selected query");
}

type Open = dyn Fn(&mut Harness<'static, PdfCraftApp>);

/// The find bar (⌘F) and the Search panel (Advanced Search) share the focus request.
fn fields() -> [(&'static str, Box<Open>); 2] {
    [
        ("find bar", Box::new(|h| h.state_mut().views[0].open_find())),
        ("Search panel", Box::new(|h| assert!(h.state_mut().execute("edit.advanced_search")))),
    ]
}

/// A second Find while the field still has the keyboard (no IME composition) selects the query
/// again, so typing replaces it instead of appending.
#[test]
fn find_again_while_the_field_has_the_keyboard_selects_the_query() {
    for (field, open) in fields() {
        let mut h = harness();
        open(&mut h);
        h.run_steps(2);
        h.event(egui::Event::Text("cat".into()));
        h.run_steps(2);
        assert_eq!(query(&h), "cat", "{field}");

        open(&mut h);
        h.run_steps(2);
        h.event(egui::Event::Text("dog".into()));
        h.run_steps(2);
        assert_eq!(query(&h), "dog", "{field}: typing replaces the selected query");
    }
}

#[test]
fn find_again_while_composing_keeps_the_query() {
    for (field, open) in fields() {
        let mut h = harness();
        open(&mut h);
        h.run_steps(2);
        h.event(egui::Event::Text("cat".into()));
        h.run_steps(2);
        h.event(egui::Event::Ime(egui::ImeEvent::Preedit { text: "ka".into(), active_range_chars: Some(0..2) }));
        h.run_steps(2);
        assert_eq!(query(&h), "catka", "{field}");

        // Selecting the query here would make the commit replace all of it.
        open(&mut h);
        h.run_steps(2);
        h.event(egui::Event::Ime(egui::ImeEvent::Commit("か".into())));
        h.run_steps(2);
        assert_eq!(query(&h), "catか", "{field}");

        // The commit ended the composition, so the next Find selects the query again.
        open(&mut h);
        h.run_steps(2);
        h.event(egui::Event::Text("dog".into()));
        h.run_steps(2);
        assert_eq!(query(&h), "dog", "{field}: typing after the composition replaces the query");
    }
}
