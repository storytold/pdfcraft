//! A printer list that arrives late must not undo what was picked while it was being read, when
//! the printer it names is the one the dialog already has.

use crate::print::{harness, printer};
use craft::print::ui::Destination;
use pdfcraft_engine::print::{PAPERS, SizeMode};
use pdfcraft_ui_egui::PrinterSetup;
use serde_json::json;
use std::sync::Arc;

#[test]
fn a_late_list_keeps_the_paper_picked_meanwhile() {
    let mut h = harness();
    h.state_mut().printer_source = Some(Arc::new(|| vec![printer("Office", true)]));
    h.open();
    h.settle();
    assert_eq!(h.destination(), Destination::Printer("Office".into()));

    // "Office" was last printed on Tabloid (#2001).
    let tabloid = PAPERS.iter().position(|(name, _)| *name == "Tabloid").unwrap();
    let setup = PrinterSetup { paper: "Tabloid".into(), size: SizeMode::Fit, custom_scale: 100.0, duplex: None };
    h.state_mut().print_draft.setups.push(("Office".into(), setup));
    h.state_mut().print_draft.apply_setup();
    assert_eq!(h.state().print_draft.paper, tabloid);
    h.close();
    // The fresh list is a job of the host that has not run yet.
    h.open();
    assert!(h.hosted().state().discovering(), "the fresh list is still being read");
    assert_eq!(h.destination(), Destination::Printer("Office".into()), "last time's list shows at once");

    // Meanwhile the person picks another paper.
    h.set("paper", json!(PAPERS[0].0));
    h.settle();
    let d = &h.state().print_draft;
    assert_eq!(h.destination(), Destination::Printer("Office".into()), "the same printer");
    assert_eq!(d.paper, 0, "the paper is as it was left, not the printer's setup again");
}
