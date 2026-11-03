use super::*;
use crate::{Edit, NewAnnotation, Shape, Style, rect_quad};
use pdfcraft_ai::library::{Added, Status};

/// One 200 × 300 pt page with two lines, the second optionally under an unapplied redaction mark.
fn two_lines(redact: bool) -> Vec<u8> {
    let content = "BT /F1 12 Tf 20 250 Td (Public figures rose.) Tj ET BT /F1 12 Tf 20 150 Td (Secret salary 90000.) Tj ET";
    let annots = if redact { " /Annots [6 0 R]" } else { "" };
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_owned(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
        format!("<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> >>{annots} >>"),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Annot /Subtype /Redact /Rect [10 140 190 170] >>".to_owned(),
    ];
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

fn protected(user: &str, owner: &str, permissions: i32) -> Arc<Vec<u8>> {
    let mut doc = pdfcraft_cos::Document::open(Arc::new(two_lines(false))).unwrap();
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: user,
        owner_password: owner,
        permissions,
        encrypt_metadata: true,
        seed: [7; 32],
    })
    .unwrap();
    Arc::new(pdfcraft_cos::write_full(&doc, &Default::default()).unwrap())
}

fn open(bytes: Vec<u8>) -> (Session, DocId) {
    let mut s = Session::new();
    let id = s.open("report.pdf", Some("/docs/report.pdf".into()), Arc::new(bytes), None).unwrap();
    (s, id)
}

fn pages_text(new: &NewDocument) -> String {
    match &new.content {
        Content::Pages(pages) => pages.iter().map(|p| p.text.as_str()).collect::<Vec<_>>().join("\n"),
        other => panic!("expected pages, got {other:?}"),
    }
}

#[test]
fn a_selection_sends_its_text_and_the_page_around_it() {
    let (s, id) = open(two_lines(false));
    // The first line is near the top of the displayed page (y down).
    let ctx = s.assistant_context(id, 0, &Selected::Region([0.0, 30.0, 200.0, 70.0])).unwrap();
    assert_eq!(ctx.text, "Public figures rose.");
    assert!(ctx.surrounding.contains("Public figures rose.") && ctx.surrounding.contains("Secret salary"), "{:?}", ctx.surrounding);
    assert_eq!((ctx.page, ctx.page_label.as_str(), ctx.title.as_str(), ctx.encrypted, ctx.redacted_glyphs), (0, "1", "report.pdf", false, 0));
    let first_word = s.assistant_context(id, 0, &Selected::Glyphs(0..6)).unwrap();
    assert_eq!(first_word.text, "Public");
    // Out-of-range glyphs are clamped, a page selection has no separate surroundings.
    let whole = s.assistant_context(id, 0, &Selected::Glyphs(0..usize::MAX)).unwrap();
    assert_eq!(whole.text, s.assistant_context(id, 0, &Selected::Page).unwrap().text);
    assert!(s.assistant_context(id, 0, &Selected::Page).unwrap().surrounding.is_empty());
    assert!(s.assistant_context(id, 0, &Selected::Region([0.0, 0.0, 1.0, 1.0])).unwrap().text.is_empty());
    assert_eq!(s.assistant_context(id, 1, &Selected::Page), Err(AssistError::NoPage(1)));
    assert_eq!(s.assistant_context(DocId(999), 0, &Selected::Page), Err(AssistError::NoDocument));
}

#[test]
fn text_under_unapplied_redaction_marks_is_never_sent() {
    let (s, id) = open(two_lines(true));
    let ctx = s.assistant_context(id, 0, &Selected::Page).unwrap();
    assert!(ctx.text.contains("Public figures rose."), "{:?}", ctx.text);
    assert!(!ctx.text.contains("Secret") && !ctx.text.contains("90000"), "{:?}", ctx.text);
    assert!(ctx.redacted_glyphs > 0);
    // Nor in the surroundings of a selection next to it, nor in the library.
    let near = s.assistant_context(id, 0, &Selected::Region([0.0, 30.0, 200.0, 70.0])).unwrap();
    assert!(!near.surrounding.contains("Secret"), "{:?}", near.surrounding);
    let lib = library_document_from_file("/docs/r.pdf", "r.pdf", Arc::new(two_lines(true)), 0).unwrap();
    assert!(!pages_text(&lib).contains("Secret"));
    assert!(pages_text(&s.library_document(id, 0).unwrap()).contains("Public"));
    assert!(!pages_text(&s.library_document(id, 0).unwrap()).contains("Secret"));
    // Without the mark the same line is sent.
    let (s, id) = open(two_lines(false));
    assert!(s.assistant_context(id, 0, &Selected::Page).unwrap().text.contains("Secret salary 90000."));
}

/// Table 22 bit 5, as Copy: a document that forbids copying gives the assistant nothing, and the
/// owner password lifts it.
#[test]
fn copy_forbidden_documents_are_refused() {
    let bytes = protected("", "owner", -1 ^ 0b1_0000);
    let mut s = Session::new();
    let id = s.open("r.pdf", None, bytes.clone(), None).unwrap();
    assert_eq!(s.assistant_context(id, 0, &Selected::Page), Err(AssistError::NotPermitted));
    assert_eq!(s.library_document(id, 0).map(|d| d.content), Ok(Content::NotPermitted));
    assert_eq!(library_document_from_file("r.pdf", "r.pdf", bytes.clone(), 0).map(|d| d.content), Ok(Content::NotPermitted));
    let owner = s.open("r.pdf", None, bytes, Some("owner")).unwrap();
    let ctx = s.assistant_context(owner, 0, &Selected::Page).unwrap();
    assert!(ctx.encrypted && ctx.text.contains("Public"));
}

#[test]
fn a_document_that_needs_a_password_is_indexed_from_its_open_copy() {
    let bytes = protected("pw", "owner", -1);
    let from_file = library_document_from_file("/docs/locked.pdf", "locked.pdf", bytes.clone(), 1).unwrap();
    assert_eq!(from_file.content, Content::NeedsPassword);
    let mut lib = Library::new();
    assert_eq!(lib.add(from_file), Ok(Added::New { status: Status::NeedsPassword, passages: 0 }));
    let mut s = Session::new();
    let id = s.open("locked.pdf", Some("/docs/locked.pdf".into()), bytes, Some("pw")).unwrap();
    let opened = s.library_document(id, 2).unwrap();
    assert_eq!(opened.doc, DocRef::of(&s.get(id).unwrap().bytes));
    assert_eq!(lib.add(opened), Ok(Added::Known { status: Status::Indexed, passages: 1 }));
    assert_eq!(lib.search("salary", 3, &[]).len(), 1);
}

#[test]
fn unreadable_files_are_errors() {
    assert!(matches!(library_document_from_file("x.pdf", "x.pdf", Arc::new(b"not a pdf".to_vec()), 0), Err(AssistError::Unreadable(_))));
    assert!(matches!(library_document_from_file("x.pdf", "x.pdf", Arc::new(Vec::new()), 0), Err(AssistError::Unreadable(_))));
}

#[test]
fn passages_name_this_document_and_the_library() {
    let (s, id) = open(two_lines(false));
    let ctx = s.assistant_context(id, 0, &Selected::Region([0.0, 30.0, 200.0, 70.0])).unwrap();
    let mine = context_passages(&ctx);
    assert_eq!(mine.len(), 2);
    assert!(mine.iter().all(|p| p.id == SourceId::ThisDocument { page: 0 }));
    assert_eq!(mine[0].title, "report.pdf, page 1 (the selection)");

    let mut lib = Library::new();
    let current = s.library_document(id, 0).unwrap();
    let current_hash = current.doc.sha256;
    lib.add(current).unwrap();
    let other = library_document_from_file("/lib/salaries.pdf", "salaries.pdf", Arc::new(two_lines(true)), 0).unwrap();
    let other_hash = other.doc.sha256;
    lib.add(other).unwrap();
    // The document being read is left out of the library results; the other one answers.
    let found = library_passages(&lib, "How did the figures rise?", Some(&ctx), &[current_hash], 6);
    assert!(!found.is_empty());
    assert!(found.iter().all(|p| p.id == SourceId::Library { sha256: other_hash, page: 0 }), "{found:?}");
    assert_eq!(found[0].title, "salaries.pdf, page 1");
    assert_eq!(s.document_sha256(id), Some(current_hash));
}

/// A mark added in this session (not one already in the file) is honoured too: the context and
/// the library read the document as it is now.
#[test]
fn text_under_a_mark_added_after_open_is_never_sent() {
    let (mut s, id) = open(two_lines(false));
    let shape = Shape::Redact { quads: vec![rect_quad([10.0, 140.0, 190.0, 170.0])], overlay: String::new(), look: Default::default() };
    let mark =
        Edit::AddAnnotation(NewAnnotation { page: 0, style: Style::default_for(&shape), shape, contents: String::new(), author: "Test".into() });
    s.apply(id, mark).unwrap();
    let ctx = s.assistant_context(id, 0, &Selected::Page).unwrap();
    assert!(!ctx.text.contains("Secret") && ctx.text.contains("Public"), "{:?}", ctx.text);
    assert!(ctx.redacted_glyphs > 0);
    assert!(!pages_text(&s.library_document(id, 0).unwrap()).contains("Secret"));
}

/// The library copy of a file indexed before it was marked for redaction is left out of the
/// questions about the open, marked document: it is the same file, and it still holds the text.
#[test]
fn earlier_copies_of_the_open_file_are_left_out_of_library_results() {
    let mut lib = Library::new();
    let before = library_document_from_file("/docs/report.pdf", "report.pdf", Arc::new(two_lines(false)), 0).unwrap();
    let before_hash = before.doc.sha256;
    lib.add(before).unwrap();
    let mut s = Session::new();
    let id = s.open("report.pdf", Some("/docs/report.pdf".into()), Arc::new(two_lines(true)), None).unwrap();
    let excluded = s.library_exclusions(id, &lib);
    assert!(excluded.contains(&before_hash), "the copy last seen at the open file's path");
    assert!(excluded.contains(&s.document_sha256(id).unwrap()));
    let ctx = s.assistant_context(id, 0, &Selected::Page).unwrap();
    let found = library_passages(&lib, "salary", Some(&ctx), &excluded, 6);
    assert!(found.is_empty(), "{found:?}");
    // A document opened from somewhere else excludes only itself.
    let elsewhere = s.open("copy.pdf", Some("/elsewhere/copy.pdf".into()), Arc::new(two_lines(true)), None).unwrap();
    assert!(!s.library_exclusions(elsewhere, &lib).contains(&before_hash));
}

#[test]
fn a_long_question_leaves_room_for_the_selection_in_the_library_query() {
    let mut lib = Library::new();
    lib.add(library_document_from_file("/lib/salaries.pdf", "salaries.pdf", Arc::new(two_lines(false)), 0).unwrap()).unwrap();
    let ctx = SelectionContext {
        page: 0,
        page_label: "1".into(),
        text: "salary".into(),
        surrounding: String::new(),
        title: "notes.pdf".into(),
        encrypted: false,
        redacted_glyphs: 0,
    };
    let question = "why ".repeat(2_000);
    assert!(!library_passages(&lib, &question, Some(&ctx), &[], 6).is_empty(), "the selection's terms still reach the search");
}

/// Pages whose text can't be read are an error when no page gave text, not "no text" (which
/// would suggest OCR for a damaged file); when other pages read, they are left out. The renderer
/// fails a page's text only when it crashes, so the reader is given here.
#[test]
fn pages_whose_text_cant_be_read() {
    let doc = inspect(Arc::new(two_lines(false)), None).unwrap();
    let mut info = doc.clone();
    info.pages = vec![doc.pages[0].clone(), doc.pages[0].clone()];
    let readable = page_text(&Arc::new(two_lines(false)), None, 0).unwrap();
    let all_fail = read_pages(&info, |_| Err("renderer crashed".into()));
    assert_eq!(all_fail, Err("page 1: renderer crashed".into()));
    let one_fails = read_pages(&info, |page| if page == 0 { Err("renderer crashed".into()) } else { Ok(Some(readable.clone())) }).unwrap();
    assert_eq!(one_fails.len(), 1);
    assert!(one_fails[0].text.contains("Public") && one_fails[0].page == 1);
    // Pages that read with no text (a scan) are "no text", not an error.
    assert_eq!(read_pages(&info, |_| Ok(Some(Arc::new(PageText::default())))).map(|p| p.len()), Ok(2));
}
