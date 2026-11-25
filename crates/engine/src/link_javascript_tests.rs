//! A link's JavaScript action (ported from #2081): it runs in the document's script realm as
//! event "Link"/"Mouse Up" with the document as its target, on the link's page; a failed one
//! commits nothing; field scripts may not open another document.

use std::sync::Arc;

use crate::{DocId, Edit, Session, js};

/// A PDF of `objs`, numbered from 1, with a cross-reference table.
fn pdf_of(objs: &[String]) -> Vec<u8> {
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

/// Two pages; page 2 has a text field `log` (4) and, as its second annotation, a link (7) whose
/// action runs `script`; the document script defines `note(s)`.
fn linked(script: &str) -> Vec<u8> {
    linked_with(script, Extra::default())
}

/// Entries added to [`linked`]'s objects, and objects added from 9 on.
#[derive(Default)]
struct Extra<'a> {
    catalog: &'a str,
    form: &'a str,
    field: &'a str,
    objects: &'a [&'a str],
}

/// [`linked`], with `extra` added.
fn linked_with(script: &str, extra: Extra) -> Vec<u8> {
    let Extra { catalog, form, field, objects } = extra;
    let mut objs = vec![
        format!(
            "<< /Type /Catalog /Pages 2 0 R {catalog} /Names << /JavaScript << /Names [(init) 8 0 R] >> >> \
             /AcroForm << /Fields [4 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> {form} >> >>"
        ),
        "<< /Type /Pages /Kids [6 0 R 3 0 R] /Count 2 /MediaBox [0 0 200 300] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 7 0 R] >>".into(),
        format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T (log) /V () /Rect [20 250 180 270] /P 3 0 R /F 4 {field} >>"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        "<< /Type /Page /Parent 2 0 R >>".into(),
        format!("<< /Type /Annot /Subtype /Link /Rect [20 20 180 60] /Border [0 0 0] /A << /S /JavaScript /JS ({script}) >> >>"),
        "<< /S /JavaScript /JS (function note\\(s\\) { getField\\('log'\\).value += s; }) >>".into(),
    ];
    objs.extend(objects.iter().map(|o| (*o).to_string()));
    pdf_of(&objs)
}

fn log(s: &Session, id: DocId) -> String {
    s.get(id).unwrap().form.iter().find(|f| f.name == "log").unwrap().value.join(",")
}

fn open(script: &str) -> (Session, DocId) {
    open_pdf(linked(script))
}

fn open_pdf(pdf: Vec<u8>) -> (Session, DocId) {
    let mut s = Session::new();
    let id = s.open("link.pdf", None, Arc::new(pdf), None).unwrap();
    s.take_js_output(id);
    (s, id)
}

/// The link reads as a JavaScript link, and its action runs as "Link"/"Mouse Up" on its page
/// with the document as target and the document scripts defined; its change is one Undo step.
#[test]
fn a_link_runs_its_javascript_in_the_document_realm() {
    let script = "note\\(event.type + ':' + event.name + ':' + \\(event.target === this\\) + ':' + this.pageNum\\); this.pageNum = 0;";
    let (mut s, id) = open(script);
    let link = s.get(id).unwrap().info.links.iter().find(|l| matches!(l.target, pdfcraft_render::LinkTarget::JavaScript(_))).cloned().unwrap();
    assert_eq!((link.page, link.index), (1, 1));
    let o = s.run_link_javascript(id, 1, 1, &[]).unwrap();
    assert_eq!(o.error, None);
    assert_eq!(log(&s, id), "Link:Mouse Up:true:1");
    assert_eq!(o.requests, [js::Request::GoToPage(0)]);
    s.undo(id).unwrap();
    assert_eq!(log(&s, id), "");
    assert!(s.run_link_javascript(id, 1, 0, &[]).is_err(), "the field is no link");
    s.set_javascript(false);
    assert!(s.run_link_javascript(id, 1, 1, &[]).is_err());
}

/// A link script that fails commits nothing: neither its field change nor its requests.
#[test]
fn a_failed_link_script_commits_nothing() {
    let (mut s, id) = open("note\\('x'\\); this.pageNum = 0; app.openDoc\\('b.pdf'\\); console.println\\('said'\\); missing\\(\\);");
    let o = s.run_link_javascript(id, 1, 1, &[]).unwrap();
    assert!(o.error.is_some());
    assert_eq!(log(&s, id), "");
    assert!(o.requests.is_empty(), "{:?}", o.requests);
    assert_eq!(o.console, ["said"], "what it said is kept");
    assert!(!s.get(id).unwrap().dirty);
}

/// A link's popup menu waits for its answer, and openDoc comes back as a request only.
#[test]
fn a_link_menu_is_answered_and_open_doc_is_only_requested() {
    let (mut s, id) = open("note\\(app.popUpMenuEx\\({cName:'A', cReturn:'a'}, {cName:'B', cReturn:'b'}\\)\\); app.openDoc\\('other.pdf'\\);");
    let o = s.run_link_javascript(id, 1, 1, &[Some("b".into())]).unwrap();
    assert_eq!(o.error, None);
    assert_eq!(log(&s, id), "b");
    assert_eq!(o.requests, [js::Request::OpenDoc("other.pdf".into())]);
}

/// The interactive start commits through `finish_javascript` with the same failure policy, and
/// a result for a document that changed meanwhile is refused.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_started_link_script_finishes_like_a_run_one() {
    let wait = |p: &js::PendingScript| loop {
        match p.receiver.recv_timeout(std::time::Duration::from_secs(10)).unwrap() {
            js::InteractiveEvent::Prompt(_, reply) => reply.send(Some("a".into())).unwrap(),
            js::InteractiveEvent::Complete(o) => break o,
        }
    };
    let (mut s, id) = open("note\\(app.popUpMenuEx\\({cName:'A', cReturn:'a'}\\)\\);");
    let p = s.start_link_javascript(id, 1, 1).unwrap();
    let o = wait(&p);
    s.finish_javascript(p, o).unwrap();
    assert_eq!(log(&s, id), "a");
    let p = s.start_link_javascript(id, 1, 1).unwrap();
    let mut o = wait(&p);
    o.error = Some("failed".into());
    s.finish_javascript(p, o).unwrap();
    assert_eq!(log(&s, id), "a", "a failed link commits nothing");
    let p = s.start_link_javascript(id, 1, 1).unwrap();
    let o = wait(&p);
    s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    assert!(s.finish_javascript(p, o).is_err(), "the document changed meanwhile");
}

/// A read-only document's link still runs: requests apply, a field change fails to commit.
#[test]
fn a_read_only_documents_link_runs() {
    let (mut s, id) = open("this.pageNum = 0;");
    s.docs.iter_mut().find(|d| d.id == id).unwrap().editor = None;
    let o = s.run_link_javascript(id, 1, 1, &[]).unwrap();
    assert_eq!((o.error, o.requests), (None, vec![js::Request::GoToPage(0)]));
}

/// A field's own script (here a format script) may not open another document: it is noted on
/// the console and dropped; a button's may.
#[test]
fn only_an_explicit_action_may_open_a_document() {
    let (mut s, id) = open("");
    s.apply(id, Edit::SetFieldScript { name: "log".into(), event: "format".into(), script: Some("app.openDoc('other.pdf');".into()) }).unwrap();
    s.take_js_output(id);
    s.apply(id, Edit::SetFieldValue { name: "log".into(), value: crate::FieldValue::Text("4".into()) }).unwrap();
    let out = s.take_js_output(id);
    assert!(out.requests.iter().all(|r| !matches!(r, js::Request::OpenDoc(_))), "{out:?}");
    assert!(out.console.iter().any(|c| c.contains("open the document other.pdf")), "{out:?}");
}

/// A link that fails before it is done leaves no timer it set and clears none: the time-out
/// never runs and the interval it cleared still does (review on #7144).
#[test]
fn a_failed_link_leaves_the_timers_as_they_were() {
    let (mut s, id) = open("app.clearInterval\\(kept\\); app.setTimeOut\\('note\\(1\\)', 5\\); missing\\(\\);");
    s.show_page(id, 1, 0.0);
    let set = s.run_javascript(id, "var kept = app.setInterval('note(2)', 1000);", None).unwrap();
    assert_eq!(set.error, None);
    assert_eq!(s.next_timer(id), Some(1000.0));
    let o = s.run_link_javascript(id, 1, 1, &[]).unwrap();
    assert!(o.error.is_some());
    assert_eq!(s.next_timer(id), Some(1000.0), "the interval stays and the time-out is gone");
    s.run_timers(id, 1000.0);
    assert_eq!(log(&s, id), "2");
}

/// A link that fails before it is done turns no layer (review on #7144).
#[test]
fn a_failed_link_turns_no_layer() {
    let pdf = linked_with(
        "this.getOCGs\\(\\)[0].state = false; missing\\(\\);",
        Extra {
            catalog: "/OCProperties << /OCGs [9 0 R] /D << /Order [9 0 R] >> >>",
            objects: &["<< /Type /OCG /Name (Red) >>"],
            ..Extra::default()
        },
    );
    let (mut s, id) = open_pdf(pdf);
    assert!(s.get(id).unwrap().info.layers[0].visible);
    let o = s.run_link_javascript(id, 1, 1, &[]).unwrap();
    assert!(o.error.is_some());
    assert!(o.layers.is_empty(), "{:?}", o.layers);
    assert!(s.get(id).unwrap().info.layers[0].visible);
}

/// In a read-only document a link's request still applies when its field change can't, and
/// the outcome says why the change wasn't made (review on #7144).
#[test]
fn a_read_only_documents_link_goes_to_its_page_after_a_field_change() {
    let (mut s, id) = open("note\\('x'\\); this.pageNum = 0;");
    s.docs.iter_mut().find(|d| d.id == id).unwrap().editor = None;
    let o = s.run_link_javascript(id, 1, 1, &[]).unwrap();
    assert_eq!(o.requests, [js::Request::GoToPage(0)]);
    let read_only = crate::EditError::ReadOnly(String::new()).to_string();
    assert_eq!(o.error.as_deref(), Some(read_only.as_str()));
    assert_eq!(log(&s, id), "");
}

/// A field's calculation that a link sets off (`calculateNow`) may not open another document,
/// as when it runs on its own; the link itself still may (review on #7144).
#[test]
fn a_calculation_a_link_runs_may_not_open_a_document() {
    let pdf = linked_with(
        "this.calculateNow\\(\\); app.openDoc\\('own.pdf'\\);",
        Extra { form: "/CO [4 0 R]", field: "/AA << /C << /S /JavaScript /JS (app.openDoc\\('calc.pdf'\\);) >> >>", ..Extra::default() },
    );
    let (mut s, id) = open_pdf(pdf);
    let o = s.run_link_javascript(id, 1, 1, &[]).unwrap();
    assert_eq!(o.error, None);
    assert_eq!(o.requests, [js::Request::OpenDoc("own.pdf".into())]);
    assert!(o.console.iter().any(|c| c.contains("open the document calc.pdf")), "{:?}", o.console);
}
