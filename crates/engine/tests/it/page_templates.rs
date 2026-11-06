//! Page templates through the session (#1429): a button's script shows a template or adds a page
//! from one as one undo step with its other changes; a field's own scripts may not.

use std::sync::Arc;

use pdfcraft_engine::{DocId, Edit, FieldValue, Session};

/// One visible page with a text field `Name` (whose validate script tries to show the template),
/// and a hidden template `More` holding the text field `Extra`.
fn form() -> Vec<u8> {
    let bodies = [
        "<< /Type /Catalog /Pages 2 0 R /Names << /Templates 6 0 R >> /AcroForm << /Fields [7 0 R 10 0 R] /DA (/Helv 0 Tf 0 g) >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>",
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 9 0 R >> >> /Annots [10 0 R] >>",
        "<< /Length 40 >>\nstream\nBT /F1 12 Tf 20 20 Td (Visible) Tj ET\nendstream",
        "<< /Type /Template /Contents 8 0 R /Resources << /Font << /F1 9 0 R >> >> /Annots [7 0 R] >>",
        "<< /Names [(More) 5 0 R] >>",
        "<< /FT /Tx /T (Extra) /Subtype /Widget /Rect [20 100 180 120] /P 5 0 R >>",
        "<< /Length 41 >>\nstream\nBT /F1 12 Tf 20 20 Td (Template) Tj ET\nendstream",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        "<< /FT /Tx /T (Name) /Subtype /Widget /Rect [20 200 180 220] /P 3 0 R /AA << /V << /S /JavaScript /JS (getTemplate('More').hidden = false;) >> >> >>",
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, b) in bodies.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{b}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", bodies.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", bodies.len() + 1).as_bytes());
    out
}

fn open() -> (Session, DocId) {
    let mut s = Session::new();
    let id = s.open("form.pdf", None, Arc::new(form()), None).expect("opens");
    (s, id)
}

fn pages(s: &Session, id: DocId) -> usize {
    s.get(id).unwrap().info.pages.len()
}

fn fields(s: &Session, id: DocId) -> Vec<String> {
    let mut names: Vec<String> = s.get(id).unwrap().form.iter().map(|f| f.name.clone()).collect();
    names.sort();
    names
}

#[test]
fn a_script_shows_a_template_as_one_undo_step() {
    let (mut s, id) = open();
    let names: Vec<(String, bool)> = s.get(id).unwrap().templates().into_iter().map(|t| (t.name, t.hidden)).collect();
    assert_eq!(names, [("More".to_string(), true)]);
    let o = s.run_javascript(id, "console.println(this.templates.length); getTemplate('More').hidden = false;", None).unwrap();
    assert_eq!((o.error, o.console), (None, vec!["1".to_string()]));
    assert_eq!(pages(&s, id), 2, "the template's page is shown");
    assert!(!s.get(id).unwrap().templates()[0].hidden);
    assert_eq!(s.get(id).unwrap().can_undo(), Some("Run JavaScript"));
    s.undo(id).unwrap();
    assert_eq!(pages(&s, id), 1, "one undo takes it out again");
}

#[test]
fn a_script_adds_pages_from_a_template_renamed_or_shared() {
    let (mut s, id) = open();
    let o = s.run_javascript(id, "this.spawnPageFromTemplate('More', 1, false, false); getTemplate('More').spawn(2, true, false);", None).unwrap();
    assert_eq!(o.error, None);
    assert_eq!(pages(&s, id), 3);
    assert_eq!(fields(&s, id), ["Extra", "Name", "P2.More.Extra"], "the renamed copy has a field of its own; the other shares Extra");
    // Saved and opened again, the pages and fields are still there.
    let bytes = s.save_bytes(id).unwrap();
    let mut again = Session::new();
    let reopened = again.open("again.pdf", None, bytes, None).unwrap();
    assert_eq!(pages(&again, reopened), 3);
    assert_eq!(fields(&again, reopened), ["Extra", "Name", "P2.More.Extra"]);
}

/// Laying a template over a page isn't done yet: the script is told on the console, and the
/// pages stay as they were.
#[test]
fn an_overlaid_spawn_is_noted_and_changes_nothing() {
    let (mut s, id) = open();
    let o = s.run_javascript(id, "getTemplate('More').spawn();", None).unwrap();
    assert_eq!(pages(&s, id), 1);
    assert!(o.console.iter().any(|l| l.contains("More") && l.contains("bOverlay")), "{:?}", o.console);
    assert!(o.requests.is_empty(), "nothing is left for the viewer: {:?}", o.requests);
}

/// A field's own scripts run unasked, so they may not change the pages; they are told so.
#[test]
fn a_field_script_may_not_change_the_pages() {
    let (mut s, id) = open();
    s.apply(id, Edit::SetFieldValue { name: "Name".into(), value: FieldValue::Text("Ada".into()) }).unwrap();
    assert_eq!(pages(&s, id), 1);
    let out = s.take_js_output(id);
    assert!(out.console.iter().any(|l| l.contains("only a button may change the pages")), "{:?}", out.console);
}

/// A script looping over `spawn` adds at most 100 pages in one run, says so once, and doesn't
/// hold the app: each page copies the document.
#[test]
fn a_spawning_loop_stops_at_its_page_budget() {
    let (mut s, id) = open();
    let o = s
        .run_javascript(id, "for (var i = 0; i < 150; i++) getTemplate('More').spawn(this.numPages, false, false); for (var j = 0; j < 50; j++) getTemplate('More').spawn();", None)
        .unwrap();
    assert_eq!(pages(&s, id), 101, "the visible page and 100 copies");
    let budget = o.console.iter().filter(|l| l.contains("at most 100 pages")).count();
    let overlays = o.console.iter().filter(|l| l.contains("bOverlay")).count();
    assert_eq!((budget, overlays), (1, 0), "said once; the overlays came after the budget was spent: {:?}", o.console);
    assert_eq!(fields(&s, id), ["Extra", "Name"], "every unrenamed copy shares Extra");
}

/// Run as another field's script (`js_run`'s `field`), a script may not change the pages
/// either: only a push button may (CodeRabbit on #5332). A name no field has is no button.
#[test]
fn a_script_run_for_a_field_that_is_no_button_may_not_change_the_pages() {
    let (mut s, id) = open();
    for target in ["Name", "Nope"] {
        let o =
            s.run_javascript(id, "getTemplate('More').hidden = false; this.spawnPageFromTemplate('More', 1, false, false);", Some(target)).unwrap();
        assert_eq!(pages(&s, id), 1, "{target}");
        let said = o.console.iter().filter(|l| l.contains("only a button may change the pages")).count();
        assert_eq!(said, 1, "{target}: said once: {:?}", o.console);
        assert!(o.requests.is_empty(), "{target}: nothing is left for the viewer: {:?}", o.requests);
    }
    assert_eq!(s.get(id).unwrap().can_undo(), None, "nothing was changed");
}
