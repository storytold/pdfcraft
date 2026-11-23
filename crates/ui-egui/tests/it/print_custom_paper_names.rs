//! A custom paper cannot take the name of a standard one: papers are looked up by name, so the
//! standard size would be found first and a printer's custom paper (#2001) would come back as it.

use crate::print::{harness, restore};
use std::sync::Arc;

#[test]
fn a_custom_paper_cannot_take_a_standard_name() {
    let mut h = harness();
    // No printers here: the list is empty as soon as it is asked for.
    h.state_mut().printer_source = Some(Arc::new(Vec::new));
    h.open();
    h.settle();
    assert!(!h.hosted().state().discovering(), "the printer list loaded");
    h.action("custom");
    let before = h.state().print_draft.paper_count();
    let per_point = h.state().print_draft.units.per_point();
    let (w, l) = (288.0 * per_point, 432.0 * per_point);
    // The name of a standard paper, in either case, is refused: Add changes nothing.
    for name in ["A4", " a4 "] {
        h.add_custom(name, w, l);
        assert_eq!(h.state().print_draft.paper_count(), before, "{name:?} is a standard paper's name");
    }
    // Any other name is accepted by the same row.
    h.add_custom("A4 label", w, l);
    assert_eq!(h.state().print_draft.paper_count(), before + 1);

    // Settings read back are checked the same way: the custom "A4" is dropped, the rest kept.
    let paper = |name: &str| serde_json::json!({ "name": name, "width": 288.0, "height": 432.0 });
    let settings = serde_json::json!({
        "print_custom_papers": [paper("A4"), paper("Label"), paper("us letter")],
    });
    let mut fresh = crate::control::new_app();
    restore(&mut fresh, &settings.to_string());
    let names: Vec<&str> = fresh.print_draft.custom_papers.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Label"]);
}
