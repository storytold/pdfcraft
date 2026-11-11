use std::sync::Arc;

use super::screen_config;
use crate::{DocId, Session};

/// A PDF of `objs` (object 1 the catalog), with its cross-reference table.
fn pdf(objs: &[String]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// A form XObject filling 100 × 100 with colour `rgb`.
fn fill(rgb: &str) -> String {
    let body = format!("{rgb} rg 0 0 100 100 re f");
    format!("<< /Type /XObject /Subtype /Form /BBox [0 0 100 100] /Length {} >>\nstream\n{body}\nendstream", body.len())
}

/// A push button widget on page 3 0 R at [50 100 150 200].
fn widget(name: &str, flags: u32, extra: &str) -> String {
    format!("<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T ({name}) /F {flags} /Rect [50 100 150 200] /P 3 0 R {extra} >>")
}

/// Two pages; on the first, an animation as the LaTeX `animate` package builds one: three
/// frames as push buttons over one another (red showing, green and blue hidden), and a widget
/// whose page-open action defines the player and starts an interval and whose page-close
/// action stops it.
fn animation_pdf() -> Vec<u8> {
    let open = "if (typeof on == 'undefined') { var on = 0; var t = null; } \
                function show(i) { getField('f' + on).display = display.hidden; on = i; getField('f' + on).display = display.visible; } \
                function step() { show((on + 1) % 3); } t = app.setInterval('step()', 100);";
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R 6 0 R 7 0 R] >> >>".into(), // 1
        "<< /Type /Pages /Kids [3 0 R 12 0 R] /Count 2 /MediaBox [0 0 200 300] >>".into(),            // 2
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R 6 0 R 7 0 R] >>".into(),                   // 3
        widget("f0", 4, "/AP << /N 8 0 R >>"),                                                        // 4
        widget("f1", 6, "/AP << /N 9 0 R >>"),                                                        // 5
        widget("f2", 6, "/AP << /N 10 0 R >>"),                                                       // 6
        widget("anim", 4, "/AA << /PO << /S /JavaScript /JS 11 0 R >> /PC << /S /JavaScript /JS (app.clearInterval\\(t\\);) >> >>"), // 7
        fill("1 0 0"),                                                                                // 8
        fill("0 1 0"),                                                                                // 9
        fill("0 0 1"),                                                                                // 10
        format!("<< /Length {} >>\nstream\n{open}\nendstream", open.len()),                           // 11
        "<< /Type /Page /Parent 2 0 R >>".into(),                                                     // 12
    ])
}

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

/// RGBA at PDF point (x, y) on `page` as the viewer draws it (1 px/pt, page height 300).
fn shown(s: &Session, id: DocId, page: usize, x: u32, y: u32) -> [u8; 4] {
    let doc = s.get(id).unwrap();
    let mut r = pdfcraft_render::PageRenderer::new(doc.display.clone(), screen_config(doc));
    let out = r.render(pdfcraft_render::RenderRequest { page, scale: 1.0, ..Default::default() });
    assert!(out.error.is_none(), "{:?}", out.error);
    let i = (((300 - y) * out.width + x) * 4) as usize;
    out.rgba[i..i + 4].try_into().unwrap()
}

/// RGBA at (x, y) on `page` as it prints or exports: the document itself.
fn printed(s: &Session, id: DocId, page: usize, x: u32, y: u32) -> [u8; 4] {
    let doc = s.get(id).unwrap();
    let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), doc.config.clone());
    let out = r.render(pdfcraft_render::RenderRequest { page, scale: 1.0, ..Default::default() });
    let i = (((300 - y) * out.width + x) * 4) as usize;
    out.rgba[i..i + 4].try_into().unwrap()
}

#[test]
fn page_actions_start_timers_that_turn_an_animations_frames_without_changing_the_document() {
    let mut s = Session::new();
    let id = s.open("animation.pdf", None, Arc::new(animation_pdf()), None).unwrap();
    let bytes = s.get(id).unwrap().bytes.clone();
    assert_eq!(shown(&s, id, 0, 100, 150), RED);
    assert_eq!(s.next_timer(id), None);
    let run = s.show_page(id, 0, 1000.0);
    assert!(run.output.errors.is_empty(), "{run:?}");
    assert_eq!(s.next_timer(id), Some(1100.0));
    assert!(s.show_page(id, 0, 1010.0).is_empty(), "the same page again runs nothing");
    assert!(s.run_timers(id, 1050.0).is_empty(), "not due yet");
    let run = s.run_timers(id, 1100.0);
    assert_eq!((run.pages.as_slice(), run.edited, run.ran), ([0].as_slice(), false, 1), "{run:?}");
    assert_eq!(shown(&s, id, 0, 100, 150), GREEN);
    assert_eq!(printed(&s, id, 0, 100, 150), RED, "printing and export draw the document");
    assert_eq!(s.next_timer(id), Some(1200.0));
    let doc = s.get(id).unwrap();
    assert!(Arc::ptr_eq(&doc.bytes, &bytes) && !doc.dirty && doc.undo_depth() == 0, "the frames turn in the view, not in the document");
    // An interval that fell behind runs once and counts on from now.
    assert_eq!(s.run_timers(id, 1450.0).ran, 1);
    assert_eq!(shown(&s, id, 0, 100, 150), BLUE);
    assert_eq!(s.next_timer(id), Some(1550.0));
    // A button runs in the same realm: the page action's functions and state are there.
    let o = s.run_javascript(id, "step();", Some("f0")).unwrap();
    assert_eq!(o.error, None);
    let o = s.run_javascript(id, "on", None).unwrap();
    assert_eq!(o.result.as_deref(), Some("0"));
    // Leaving the page runs its close action, which clears the interval.
    s.show_page(id, 1, 1500.0);
    assert_eq!(s.next_timer(id), None);
    // With JavaScript off, page actions don't run.
    s.set_javascript(false);
    assert!(s.show_page(id, 0, 1600.0).is_empty());
    assert_eq!(s.next_timer(id), None);
}

#[test]
fn a_button_that_shows_a_frame_changes_the_document_and_the_views_frame_gives_way() {
    let mut s = Session::new();
    let id = s.open("animation.pdf", None, Arc::new(animation_pdf()), None).unwrap();
    s.show_page(id, 0, 0.0);
    s.run_timers(id, 100.0);
    assert_eq!(shown(&s, id, 0, 100, 150), GREEN);
    s.run_javascript(id, "step();", Some("f0")).unwrap();
    assert_eq!(shown(&s, id, 0, 100, 150), BLUE);
    assert!(s.get(id).unwrap().dirty, "a button's change is the document's");
    assert_eq!(printed(&s, id, 0, 100, 150), BLUE);
    s.run_timers(id, 200.0);
    assert_eq!(shown(&s, id, 0, 100, 150), RED);
}

#[test]
fn a_timer_runs_unasked_so_its_alerts_and_requests_go_to_the_console() {
    let open = "app.setTimeOut('app.alert(\"hi\"); this.print(); app.launchURL(\"https://example.com\");', 50);";
    let mut s = Session::new();
    let id = s.open("t.pdf", None, Arc::new(timer_pdf(open)), None).unwrap();
    assert!(s.show_page(id, 0, 0.0).output.errors.is_empty());
    let run = s.run_timers(id, 50.0);
    assert_eq!(run.ran, 1);
    assert!(run.output.alerts.is_empty() && run.output.requests.is_empty(), "{run:?}");
    assert!(run.output.console.iter().any(|l| l.contains("hi")), "{run:?}");
    assert_eq!(s.next_timer(id), None, "a time-out runs once");
}

#[test]
fn a_fresh_realm_has_no_timers_and_a_timer_writing_a_value_is_one_undoable_edit() {
    let open = "app.setInterval('getField(\"f0\").value = \"x\";', 10);";
    let mut s = Session::new();
    let id = s.open("t.pdf", None, Arc::new(timer_pdf(open)), None).unwrap();
    s.show_page(id, 0, 0.0);
    let run = s.run_timers(id, 10.0);
    assert!(run.edited && run.output.errors.is_empty(), "{run:?}");
    assert_eq!(s.get(id).unwrap().undo_depth(), 1);
    assert!(s.next_timer(id).is_some());
    s.invalidate_javascript(id);
    assert_eq!(s.next_timer(id), None, "the timers went with the realm");
}

/// One page with a text field f0 whose page-open action is `open`.
fn timer_pdf(open: &str) -> Vec<u8> {
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R] /AA << /O 5 0 R >> >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (f0) /Rect [50 100 150 200] /P 3 0 R >>".into(),
        "<< /S /JavaScript /JS 6 0 R >>".into(),
        format!("<< /Length {} >>\nstream\n{open}\nendstream", open.len()),
    ])
}

/// An animation as `animate` builds one by default (method=icon): hidden frames are buttons
/// holding the icons, and the player gives the one showing button a frame's icon.
#[test]
fn a_timer_giving_a_button_another_icon_draws_that_icon_in_its_place() {
    let open = "if (typeof on == 'undefined') { var on = 0; var fr = [getField('f0').buttonGetIcon(), getField('f1').buttonGetIcon()]; } \
                app.setInterval('on = 1 - on; getField(\"anim\").buttonSetIcon(fr[on]);', 100);";
    let out = pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R 6 0 R] >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R 6 0 R] >>".into(),
        widget("anim", 4, "/AP << /N 7 0 R >> /MK << /I 7 0 R >> /AA << /PO << /S /JavaScript /JS 9 0 R >> >>"),
        widget("f0", 6, "/AP << /N 7 0 R >> /MK << /I 7 0 R >>"),
        widget("f1", 6, "/AP << /N 8 0 R >> /MK << /I 8 0 R >>"),
        fill("1 0 0"),
        fill("0 1 0"),
        format!("<< /Length {} >>\nstream\n{open}\nendstream", open.len()),
    ]);
    let mut s = Session::new();
    let id = s.open("icons.pdf", None, Arc::new(out), None).unwrap();
    assert_eq!(shown(&s, id, 0, 100, 150), RED);
    assert!(s.show_page(id, 0, 0.0).output.errors.is_empty());
    let run = s.run_timers(id, 100.0);
    assert_eq!((run.pages.as_slice(), run.output.errors.len()), ([0].as_slice(), 0), "{run:?}");
    assert_eq!(shown(&s, id, 0, 100, 150), GREEN);
    assert_eq!(printed(&s, id, 0, 100, 150), RED);
    s.run_timers(id, 200.0);
    assert_eq!(shown(&s, id, 0, 100, 150), RED, "its own icon again");
    assert!(!s.get(id).unwrap().dirty);
}

#[test]
fn get_ocgs_turns_a_layer_in_the_view() {
    let open = "this.getOCGs()[0].state = false;";
    let out = pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [5 0 R] /D << /Order [5 0 R] >> >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Properties << /L1 5 0 R >> >> /AA << /O << /S /JavaScript /JS 6 0 R >> >> >>"
            .into(),
        "<< /Length 41 >>\nstream\n/OC /L1 BDC 1 0 0 rg 0 0 200 300 re f EMC\nendstream".into(),
        "<< /Type /OCG /Name (Red) >>".into(),
        format!("<< /Length {} >>\nstream\n{open}\nendstream", open.len()),
    ]);
    let mut s = Session::new();
    let id = s.open("layers.pdf", None, Arc::new(out), None).unwrap();
    assert_eq!(shown(&s, id, 0, 100, 150), RED);
    let run = s.show_page(id, 0, 0.0);
    assert!(run.layers && run.output.errors.is_empty(), "{run:?}");
    assert!(!s.get(id).unwrap().info.layers[0].visible);
    assert_eq!(shown(&s, id, 0, 100, 150), [255, 255, 255, 255]);
    assert!(!s.get(id).unwrap().dirty);
}

/// A page action whose document edit is refused (here: past the reset limit) shows nothing it
/// did either. The frame it showed stays hidden.
#[test]
fn a_page_action_whose_edit_is_refused_shows_nothing() {
    let open = "getField('f1').display = display.visible; for (var i = 0; i < 70; i++) this.resetForm();";
    let out = pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R] >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R] /AA << /O << /S /JavaScript /JS 8 0 R >> >> >>".into(),
        widget("f0", 4, "/AP << /N 6 0 R >>"),
        widget("f1", 6, "/AP << /N 7 0 R >>"),
        fill("1 0 0"),
        fill("0 1 0"),
        format!("<< /Length {} >>\nstream\n{open}\nendstream", open.len()),
    ]);
    let mut s = Session::new();
    let id = s.open("refused.pdf", None, Arc::new(out), None).unwrap();
    let run = s.show_page(id, 0, 0.0);
    assert!(!run.output.errors.is_empty(), "the edit was refused: {run:?}");
    assert!(run.pages.is_empty(), "{run:?}");
    assert_eq!(shown(&s, id, 0, 100, 150), RED, "the frame it showed stays hidden");
}

/// A field's calculation reads the document's layers (`getOCGs`), as page actions and buttons do.
#[test]
fn field_calculations_read_the_documents_layers() {
    let calc = "var l = this.getOCGs(); event.value = l ? String(l.length) : 'none';";
    let out = pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R] /CO [5 0 R] >> /OCProperties << /OCGs [6 0 R] /D << /Order [6 0 R] >> >> >>"
            .into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R] >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (a) /Rect [10 10 90 30] /P 3 0 R >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (n) /Rect [10 40 90 60] /P 3 0 R /AA << /C << /S /JavaScript /JS 7 0 R >> >> >>".into(),
        "<< /Type /OCG /Name (Red) >>".into(),
        format!("<< /Length {} >>\nstream\n{calc}\nendstream", calc.len()),
    ]);
    let mut s = Session::new();
    let id = s.open("calc.pdf", None, Arc::new(out), None).unwrap();
    s.apply(id, crate::Edit::SetFieldValue { name: "a".into(), value: crate::FieldValue::Text("1".into()) }).unwrap();
    let n = s.get(id).unwrap().form.iter().find(|f| f.name == "n").unwrap().value.clone();
    assert_eq!(n, ["1"], "the calculation saw the document's one layer");
}

/// The viewer's hit test asks which fields the scripts show (`shown_by_scripts`): a frame they
/// showed, and one they hid.
#[test]
fn the_view_says_which_fields_the_scripts_show() {
    let mut s = Session::new();
    let id = s.open("animation.pdf", None, Arc::new(animation_pdf()), None).unwrap();
    assert_eq!(s.get(id).unwrap().shown_by_scripts("f1"), None);
    s.show_page(id, 0, 0.0);
    s.run_timers(id, 100.0);
    let doc = s.get(id).unwrap();
    assert_eq!((doc.shown_by_scripts("f0"), doc.shown_by_scripts("f1"), doc.shown_by_scripts("anim")), (Some(false), Some(true), None));
}

/// With a cap of none, timers due only move the clock on; they run at the next call.
#[test]
fn a_cap_of_no_timer_runs_only_moves_the_clock() {
    let mut s = Session::new();
    let id = s.open("animation.pdf", None, Arc::new(animation_pdf()), None).unwrap();
    s.show_page(id, 0, 0.0);
    let run = s.run_timers_at_most(id, 100.0, 0);
    assert_eq!(run.ran, 0);
    assert_eq!((s.script_clock(id), s.next_timer(id)), (Some(100.0), Some(100.0)));
    assert_eq!(s.run_timers_at_most(id, 100.0, 1).ran, 1);
}

/// A script refused before it runs (too long) leaves the page's timers running.
#[test]
fn a_script_refused_before_it_runs_keeps_the_pages_timers() {
    let mut s = Session::new();
    let id = s.open("t.pdf", None, Arc::new(timer_pdf("app.setInterval('1', 10);")), None).unwrap();
    s.show_page(id, 0, 0.0);
    assert_eq!(s.next_timer(id), Some(10.0));
    let refused = s.run_javascript(id, &"1;".repeat(256 * 1024), None).expect("runs");
    assert!(refused.error.is_some(), "the script is refused");
    assert_eq!(s.next_timer(id), Some(10.0), "and the interval still runs");
}
