//! Contributor-original pages. Added marks must not inherit an unfinished hidden layer.
use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, Object, SaveOptions, Stream, write_incremental};
use pdfcraft_engine::{AddedText, Edit, HeaderFooter, Session, Watermark};
use pdfcraft_render::{PageRenderer, RenderRequest, RequestKind};

const VISIBLE: &str = "BT /F1 18 Tf 72 700 Td (Visible text) Tj ET\n";
const HIDDEN: &str = "BT /F1 18 Tf 72 600 Td (Hidden note) Tj ET\n";

fn fixture(parts: &[String]) -> Arc<Vec<u8>> {
    let mut doc = Document::new_empty();
    let pages = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type1"));
    font.set(b"BaseFont".to_vec(), Object::name("Helvetica"));
    let mut fonts = Dict::new();
    fonts.set(b"F1".to_vec(), Object::Ref(doc.add(font)));
    let mut group = Dict::new();
    group.set(b"Type".to_vec(), Object::name("OCG"));
    group.set(b"Name".to_vec(), Object::String(pdfcraft_cos::PdfString::text("Hidden layer")));
    let group = doc.add(group);
    let mut properties = Dict::new();
    properties.set(b"L1".to_vec(), Object::Ref(group));
    let mut resources = Dict::new();
    resources.set(b"Font".to_vec(), Object::Dict(fonts));
    resources.set(b"Properties".to_vec(), Object::Dict(properties));
    let mut config = Dict::new();
    config.set(b"OFF".to_vec(), Object::Array(vec![Object::Ref(group)]));
    let mut oc = Dict::new();
    oc.set(b"OCGs".to_vec(), Object::Array(vec![Object::Ref(group)]));
    oc.set(b"D".to_vec(), Object::Dict(config));
    doc.update_dict(doc.root().unwrap(), |d| d.set(b"OCProperties".to_vec(), Object::Dict(oc))).unwrap();
    let streams = parts.iter().map(|s| Object::Ref(doc.add(Object::Stream(Stream::from_raw(Dict::new(), s.as_bytes().to_vec()))))).collect();
    let mut page = Dict::new();
    page.set(b"Type".to_vec(), Object::name("Page"));
    page.set(b"Parent".to_vec(), Object::Ref(pages));
    page.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 612.into(), 792.into()]));
    page.set(b"Resources".to_vec(), Object::Dict(resources));
    page.set(b"Contents".to_vec(), Object::Array(streams));
    let page = doc.add(page);
    doc.update_dict(pages, |d| {
        d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
        d.set(b"Count".to_vec(), Object::Int(1));
    })
    .unwrap();
    Arc::new(write_incremental(&doc, &SaveOptions::default()).unwrap())
}

fn render(bytes: Arc<Vec<u8>>) -> (Vec<u8>, String) {
    let mut renderer = PageRenderer::new(bytes, Default::default());
    let pixels = renderer.render(RenderRequest { page: 0, scale: 1.0, ..Default::default() });
    assert!(pixels.error.is_none(), "{:?}", pixels.error);
    let text = renderer.render(RenderRequest { page: 0, kind: RequestKind::Text, ..Default::default() });
    assert!(text.error.is_none(), "{:?}", text.error);
    let text = text.text.unwrap();
    (pixels.rgba.to_vec(), text.text_of(0..text.glyphs.len()))
}

fn edit(kind: &str) -> Edit {
    match kind {
        "HEADER" | "000001" => {
            let mut settings = HeaderFooter::default();
            if kind == "HEADER" {
                settings.text[1] = "HEADER".into();
            } else {
                settings.text[5] = "<<Bates Number#6#1##>>".into();
            }
            Edit::AddHeaderFooter { pages: vec![0], settings, replace: true }
        }
        "WATERMARK" => Edit::AddWatermark {
            pages: vec![0],
            settings: Watermark { text: kind.into(), opacity: 1.0, rotation: 0.0, font_size: 18.0, ..Default::default() },
            replace: true,
            file: None,
        },
        "ADDED" => Edit::AddText { page: 0, text: AddedText { text: kind.into(), rect: [72.0, 300.0, 250.0, 330.0], ..Default::default() } },
        _ => panic!("unknown mark"),
    }
}

fn add_and_save(input: Arc<Vec<u8>>, kind: &str) -> Arc<Vec<u8>> {
    let mut session = Session::new();
    let id = session.open("fixture.pdf", None, input.clone(), None).unwrap();
    session.apply(id, edit(kind)).unwrap();
    let saved = session.save_bytes(id).unwrap();
    assert!(saved.starts_with(&input), "incremental save preserves the original file bytes");
    assert_original_streams(&input, &saved);
    let mut reopened = Session::new();
    let id = reopened.open("saved.pdf", None, saved.clone(), None).unwrap();
    assert_eq!(reopened.save_bytes(id).unwrap().as_slice(), saved.as_slice());
    saved
}

#[test]
fn all_four_mark_paths_remain_visible_after_saved_unfinished_marked_content() {
    let mut failures = Vec::new();
    for (case, opening, hidden) in [("hidden", "/OC /L1 BDC\n", true), ("artifact", "/Artifact BMC\n", false), ("balanced", "/OC /L1 BDC\n", true)] {
        let closed = fixture(&[format!("{VISIBLE}{opening}{HIDDEN}EMC\n")]);
        let input = if case == "balanced" { closed.clone() } else { fixture(&[format!("{VISIBLE}{opening}{HIDDEN}")]) };
        let original = render(input.clone());
        assert!(original.1.contains("Visible text"));
        assert_eq!(original.1.contains("Hidden note"), !hidden);
        for kind in ["HEADER", "000001", "WATERMARK", "ADDED"] {
            let saved = add_and_save(input.clone(), kind);

            let actual = render(saved);
            let expected_bytes = add_and_save(closed.clone(), kind);

            let expected = render(expected_bytes);
            let mut wanted = vec!["Visible text", kind];
            if !hidden {
                wanted.push("Hidden note");
            }
            wanted.sort_unstable();
            let pass = actual.0 == expected.0 && actual.1 == expected.1 && lines(&actual.1) == wanted;
            eprintln!("{case}/{kind}: visible={} closed-control-pixels={}", actual.1.contains(kind), actual.0 == expected.0);
            if !pass {
                failures.push(format!("{case}/{kind}: {:?}", actual.1));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn stream_list(doc: &Document) -> Vec<Object> {
    let page = pdfcraft_model::pages(doc).remove(0);
    let contents = doc.resolve(page.dict.get(b"Contents").unwrap());
    match &*contents {
        Object::Array(items) => items.clone(),
        _ => vec![page.dict.get(b"Contents").unwrap().clone()],
    }
}

fn assert_original_streams(before: &Arc<Vec<u8>>, after: &Arc<Vec<u8>>) {
    let original = Document::open(before.clone()).unwrap();
    let saved = Document::open(after.clone()).unwrap();
    let saved_list = stream_list(&saved);
    for reference in stream_list(&original) {
        assert!(saved_list.contains(&reference), "the original stream stays on the page");
        assert_eq!(original.resolve(&reference), saved.resolve(&reference), "original encoded stream object remains unchanged");
    }
}

fn lines(text: &str) -> Vec<&str> {
    let mut lines: Vec<_> = text.lines().filter(|line| !line.is_empty()).collect();
    lines.sort_unstable();
    lines
}

#[test]
fn nested_stream_edges_and_malformed_graphics_keep_hidden_content_hidden() {
    let cases: Vec<(&str, Vec<String>, usize)> = vec![
        ("nested", vec![format!("{VISIBLE}/OC /L1 BDC /Artifact BMC {HIDDEN}")], 2),
        ("split operands", vec![format!("{VISIBLE}/OC"), format!("/L1 BDC {HIDDEN}")], 1),
        ("split close", vec![format!("{VISIBLE}/OC /L1 BDC {HIDDEN}"), "EMC\n".into()], 0),
        ("trailing comment", vec![format!("{VISIBLE}/OC /L1 BDC {HIDDEN}% EMC")], 1),
        ("cross-stream comment", vec![format!("{VISIBLE}/OC /L1 BDC {HIDDEN}% EMC"), "EMC BMC\n".into()], 1),
        ("unclosed graphics", vec![format!("{VISIBLE}/OC /L1 BDC {HIDDEN}q")], 1),
        ("balanced graphics open marked", vec![format!("q {VISIBLE}/OC /L1 BDC {HIDDEN}Q")], 1),
        ("malformed graphics", vec![format!("Q {VISIBLE}/OC /L1 BDC {HIDDEN}")], 1),
        ("invalid begins", vec![format!("{VISIBLE}/OC /L1 BDC {HIDDEN}123 BMC /Artifact BDC")], 1),
        ("extra begins", vec![format!("{VISIBLE}123 /OC /L1 BDC {HIDDEN}123 /Artifact BMC")], 2),
        ("extra close", vec![format!("{VISIBLE}/OC /L1 BDC {HIDDEN}123 EMC")], 0),
        ("underflow", vec![format!("EMC {VISIBLE}/OC /L1 BDC {HIDDEN}")], 1),
        ("operator strings", vec![format!("{VISIBLE}/OC /L1 BDC {HIDDEN}/Note <</Text (BMC BDC EMC)>> DP")], 1),
        ("inline samples", vec![format!("{VISIBLE}/OC /L1 BDC {HIDDEN}BI /W 11 /H 1 /BPC 8 /CS /G ID BMC BDC EMC EI\n")], 1),
    ];
    for (case, parts, close_count) in cases {
        let input = fixture(&parts);
        let mut closed = parts.clone();
        closed.push(format!("\n{}", "EMC\n".repeat(close_count)));
        let actual = render(add_and_save(input, "HEADER"));
        let expected = render(add_and_save(fixture(&closed), "HEADER"));
        assert_eq!(lines(&actual.1), lines("Visible text\nHEADER"), "{case}");
        assert!(actual.0 == expected.0, "{case}: pixels equal the explicitly closed control");
    }
}

#[test]
fn replacing_and_removing_marks_restores_the_original_stream_list() {
    let input = fixture(&[format!("{VISIBLE}/OC /L1 BDC {HIDDEN}")]);
    let original = Document::open(input.clone()).unwrap();
    for kind in ["HEADER", "000001", "WATERMARK"] {
        let mut session = Session::new();
        let mut id = session.open("fixture.pdf", None, input.clone(), None).unwrap();
        for _ in 0..3 {
            session.apply(id, edit(kind)).unwrap();
            let saved = session.save_bytes(id).unwrap();
            let page = Document::open(saved.clone()).unwrap();
            assert_eq!(stream_list(&page).len(), 4, "{kind}: updating does not add wrappers");
            assert_original_streams(&input, &saved);
            assert_eq!(lines(&render(saved.clone()).1), lines(&format!("Visible text\n{kind}")));
            session = Session::new();
            id = session.open("saved.pdf", None, saved, None).unwrap();
        }
        let mark_kind = if kind == "WATERMARK" { pdfcraft_edit::MarkKind::Watermark } else { pdfcraft_edit::MarkKind::HeaderFooter };
        session.apply(id, Edit::RemoveMarks { kind: mark_kind }).unwrap();
        let saved = session.save_bytes(id).unwrap();
        let page = Document::open(saved.clone()).unwrap();
        assert_eq!(stream_list(&page), stream_list(&original), "{kind}: removal restores original references");
        assert_original_streams(&input, &saved);
        assert!(render(saved) == render(input.clone()), "{kind}: removal restores original pixels and text");
        session.undo(id).unwrap();
        assert_eq!(lines(&render(session.save_bytes(id).unwrap()).1), lines(&format!("Visible text\n{kind}")));
        session.redo(id).unwrap();
        assert!(render(session.save_bytes(id).unwrap()) == render(input.clone()), "redo removal restores the original rendering");
    }
}

#[test]
fn added_update_delete_and_readd_preserve_existing_wrapper_behavior() {
    let input = fixture(&[format!("{VISIBLE}/OC /L1 BDC {HIDDEN}")]);
    let mut session = Session::new();
    let mut id = session.open("fixture.pdf", None, input.clone(), None).unwrap();
    for _ in 0..3 {
        session.apply(id, edit("ADDED")).unwrap();
        assert_eq!(session.get(id).unwrap().added().len(), 1);
        let text = AddedText { text: "UPDATED".into(), rect: [72.0, 300.0, 250.0, 330.0], ..Default::default() };
        session.apply(id, Edit::UpdateContent { page: 0, index: 0, content: pdfcraft_edit::Content::Text(text) }).unwrap();
        let saved = session.save_bytes(id).unwrap();
        assert_eq!(lines(&render(saved.clone()).1), lines("Visible text\nUPDATED"));
        assert_eq!(stream_list(&Document::open(saved.clone()).unwrap()).len(), 4);
        assert_original_streams(&input, &saved);
        session = Session::new();
        id = session.open("saved.pdf", None, saved, None).unwrap();
        session.apply(id, Edit::DeleteContent { page: 0, index: 0 }).unwrap();
        let saved = session.save_bytes(id).unwrap();
        assert_eq!(stream_list(&Document::open(saved.clone()).unwrap()).len(), 3, "Added deletion retains the established wrappers");
        assert!(render(saved.clone()) == render(input.clone()), "deleting Added restores the original rendering");
        assert_original_streams(&input, &saved);
        session = Session::new();
        id = session.open("saved.pdf", None, saved, None).unwrap();
    }
}

#[test]
fn an_existing_behind_watermark_does_not_inherit_or_reveal_hidden_content() {
    let input = fixture(&[format!("{VISIBLE}/OC /L1 BDC {HIDDEN}")]);
    let mut session = Session::new();
    let id = session.open("fixture.pdf", None, input.clone(), None).unwrap();
    session
        .apply(
            id,
            Edit::AddWatermark {
                pages: vec![0],
                settings: Watermark { text: "BEHIND".into(), behind: true, font_size: 18.0, opacity: 1.0, rotation: 0.0, ..Default::default() },
                replace: false,
                file: None,
            },
        )
        .unwrap();
    session.apply(id, edit("HEADER")).unwrap();
    let saved = session.save_bytes(id).unwrap();
    assert_eq!(lines(&render(saved.clone()).1), lines("Visible text\nBEHIND\nHEADER"));
    assert_original_streams(&input, &saved);
}

#[test]
fn new_mark_closes_hidden_content_appended_after_an_old_wrapper() {
    let bytes = fixture(&["q %PdfCraft\n".into(), VISIBLE.into(), "Q %PdfCraft\n".into(), format!("/OC /L1 BDC {HIDDEN}")]);
    let mut doc = Document::open(bytes).unwrap();
    let list = stream_list(&doc);
    for index in [0, 2] {
        let reference = list[index].as_ref().unwrap();
        let Object::Stream(mut stream) = doc.get(reference).as_ref().clone() else { panic!("stream") };
        stream.dict.set(b"PCMark".to_vec(), Object::name("Wrap"));
        doc.set(reference, Object::Stream(stream));
    }
    let input = Arc::new(write_incremental(&doc, &SaveOptions::default()).unwrap());
    let saved = add_and_save(input, "HEADER");
    assert_eq!(lines(&render(saved).1), lines("Visible text\nHEADER"));
}
