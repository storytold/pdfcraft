use super::{card, drag, expand, fixture_sized, grid_of, page_card, texts_of};
use egui::{Key, Modifiers, vec2};
use egui_kittest::kittest::Queryable;

fn whole_file_between_pages(single: bool) {
    let shots = std::env::var_os("PDFCRAFT_COMBINE_INSERT_SHOTS").is_some();
    let _gpu = shots.then(crate::gpu);
    let mut h = grid_of(
        vec![("a.pdf", fixture_sized(5, 200.0, 300.0)), ("b.pdf", fixture_sized(3, 210.0, 310.0)), ("c.pdf", fixture_sized(1, 220.0, 320.0))],
        vec2(1400.0, 900.0),
    );
    expand(&mut h, "a.pdf");
    let original = h.state().combine_parts_shown();
    let source = card(&h, if single { "c.pdf" } else { "b.pdf" });
    let target = card(&h, &page_card(4, "a.pdf"));
    drag(&mut h, source.center(), target.center() - vec2(target.width() * 0.3, 0.0));
    let inserted = h.state().combine_parts_shown();
    let expected = if single {
        vec![("a.pdf".into(), vec![0, 1, 2]), ("c.pdf".into(), vec![0]), ("a.pdf".into(), vec![3, 4]), ("b.pdf".into(), vec![0, 1, 2])]
    } else {
        vec![("a.pdf".into(), vec![0, 1, 2]), ("b.pdf".into(), vec![0, 1, 2]), ("a.pdf".into(), vec![3, 4]), ("c.pdf".into(), vec![0])]
    };
    assert_eq!(inserted, expected, "all of the collapsed file is inserted before a's page 4");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert_eq!(h.state().combine_parts_shown(), original, "one Undo restores both whole files and the page order");
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    h.run_steps(3);
    assert_eq!(h.state().combine_parts_shown(), inserted, "one Redo restores the insertion");
    if shots {
        h.render().unwrap().save(format!("/tmp/5768-insert-{}.png", if single { "single" } else { "multi" })).unwrap();
    }
    h.get_by_label("Combine").click();
    h.run_steps(4);
    let app = h.state();
    assert_eq!(app.views.len(), 1, "the combined PDF opens");
    let doc = app.session.get(app.views[0].id).unwrap();
    let widths = doc.info.pages.iter().map(|p| p.width).collect::<Vec<_>>();
    assert_eq!(
        widths,
        if single {
            vec![200.0, 200.0, 200.0, 220.0, 200.0, 200.0, 210.0, 210.0, 210.0]
        } else {
            vec![200.0, 200.0, 200.0, 210.0, 210.0, 210.0, 200.0, 200.0, 220.0]
        },
        "the actual PDF keeps each source at the drop position"
    );
    assert_eq!(
        texts_of(app, 0),
        if single {
            vec!["Page 1", "Page 2", "Page 3", "Page 1", "Page 4", "Page 5", "Page 1", "Page 2", "Page 3"]
        } else {
            vec!["Page 1", "Page 2", "Page 3", "Page 1", "Page 2", "Page 3", "Page 4", "Page 5", "Page 1"]
        }
    );
}
#[test]
fn a_collapsed_multi_page_file_inserts_between_pages() {
    whole_file_between_pages(false);
}
#[test]
fn a_single_page_file_inserts_between_pages() {
    whole_file_between_pages(true);
}

#[test]
fn selected_ranged_files_insert_in_original_order_from_before_target() {
    let mut h = grid_of(
        vec![("b.pdf", fixture_sized(3, 210.0, 310.0)), ("c.pdf", fixture_sized(1, 220.0, 320.0)), ("a.pdf", fixture_sized(5, 200.0, 300.0))],
        vec2(1400.0, 900.0),
    );
    h.state_mut().combine_draft[0].range = "3, 1".into();
    h.run_steps(2);
    expand(&mut h, "a.pdf");
    h.get_by_label("b.pdf").click();
    h.run_steps(1);
    h.get_by_label("c.pdf").click_modifiers(Modifiers::COMMAND);
    h.run_steps(2);
    assert_eq!(h.state().combine_selection(), [0, 1]);
    let original = h.state().combine_parts_shown();
    let source = card(&h, "c.pdf");
    let target = card(&h, &page_card(4, "a.pdf"));
    drag(&mut h, source.center(), target.center() - vec2(target.width() * 0.3, 0.0));
    let inserted = h.state().combine_parts_shown();
    assert_eq!(
        inserted,
        vec![("a.pdf".to_string(), vec![0, 1, 2]), ("b.pdf".into(), vec![2, 0]), ("c.pdf".into(), vec![0]), ("a.pdf".into(), vec![3, 4])]
    );
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert_eq!(h.state().combine_parts_shown(), original, "one Undo restores the original selected ranges and order");
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    h.run_steps(3);
    assert_eq!(h.state().combine_parts_shown(), inserted);
    h.get_by_label("Combine").click();
    h.run_steps(4);
    assert_eq!(texts_of(h.state(), 0), ["Page 1", "Page 2", "Page 3", "Page 3", "Page 1", "Page 1", "Page 4", "Page 5"]);
}
