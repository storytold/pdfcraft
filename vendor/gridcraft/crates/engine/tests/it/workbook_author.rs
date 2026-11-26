//! Regression tests for the default author of newly created workbooks.

use gridcraft_engine::{DocState, Session, Trust, io, model::Workbook};

#[test]
fn new_workbook_stamps_the_configured_user_name_at_creation() {
    let mut session = Session::new();

    session.prefs.user_name = "Ada Lovelace".into();
    let first = session.new_workbook();

    assert_eq!(session.documents()[first].wb.props.author, "Ada Lovelace");

    // Changing the preference must not rewrite an existing workbook.
    session.prefs.user_name = "Grace Hopper".into();

    assert_eq!(session.documents()[first].wb.props.author, "Ada Lovelace");

    // A new workbook uses the preference as it exists at creation.
    let second = session.new_workbook();

    assert_eq!(session.documents()[second].wb.props.author, "Grace Hopper");

    assert_eq!(session.documents()[first].wb.props.author, "Ada Lovelace");
}

#[test]
fn opening_and_resaving_preserves_the_existing_workbook_author() {
    // An existing XLSX has its own author metadata.
    let mut source = Workbook::new();
    source.props.author = "Original Spreadsheet Author".into();

    let bytes = io::save_bytes(&source, "source.xlsx").unwrap();
    let (opened, _) = io::open_bytes("source.xlsx", &bytes).unwrap();

    let mut session = Session::new();
    session.prefs.user_name = "Current GridCraft User".into();

    session.add_document(DocState::new(opened, None, "source".into(), Trust::FromFile));

    // Opening an existing workbook must not replace its author.
    assert_eq!(session.doc().unwrap().wb.props.author, "Original Spreadsheet Author");

    // The original author must survive another XLSX save and reopen.
    let saved = io::save_bytes(&session.doc().unwrap().wb, "saved.xlsx").unwrap();

    let (reopened, _) = io::open_bytes("saved.xlsx", &saved).unwrap();

    assert_eq!(reopened.props.author, "Original Spreadsheet Author");
}
