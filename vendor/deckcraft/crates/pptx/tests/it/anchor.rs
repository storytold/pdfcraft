//! Text anchoring of shapes without an `anchor` of their own: the model centres a shape's text,
//! a file without one puts it at the top, whatever the theme's shape defaults say.

use std::io::{Cursor, Read, Write};

use deckcraft_model::text::Anchor;
use deckcraft_model::{Presentation, ShapeKind};
use zip::write::SimpleFileOptions;

fn sample() -> Presentation {
    let mut s = deckcraft_engine::Session::new();
    deckcraft_engine::sample::open_sample(&mut s).expect("sample");
    s.doc().expect("doc").doc.as_ref().clone()
}

/// Every part of `bytes` with `edit` applied to the XML ones.
fn rewrite(bytes: &[u8], edit: impl Fn(&str, String) -> String) -> Vec<u8> {
    let mut src = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for i in 0..src.len() {
        let mut f = src.by_index(i).expect("entry");
        let name = f.name().to_string();
        let mut data = Vec::new();
        f.read_to_end(&mut data).expect("read");
        if name.ends_with(".xml") {
            data = edit(&name, String::from_utf8(data).expect("utf8")).into_bytes();
        }
        out.start_file(name, SimpleFileOptions::default()).expect("start");
        out.write_all(&data).expect("write");
    }
    out.finish().expect("finish").into_inner()
}

/// Anchors of the shapes with text that aren't placeholders, text boxes or tables.
fn anchors(p: &Presentation) -> Vec<Option<Anchor>> {
    p.slides
        .iter()
        .flat_map(|s| s.shapes.iter())
        .filter(|s| s.ph.is_none() && !s.text_box && !matches!(s.kind, ShapeKind::Table(_)))
        .filter_map(|s| s.text.as_ref().map(|t| t.body.anchor))
        .collect()
}

#[test]
fn shapes_keep_their_middle_anchor_and_files_without_one_anchor_at_the_top() {
    let p = sample();
    assert!(anchors(&p).contains(&None), "the sample has shapes with the model's default anchor");
    let bytes = deckcraft_pptx::export(&p).expect("export");

    // Written explicitly, so PowerPoint (which reads a missing anchor as top) centres them too.
    let back = deckcraft_pptx::import(&bytes).expect("import");
    let a = anchors(&back);
    assert_eq!(a, anchors(&p), "the middle anchor survives (as the model's default)");

    // The same file without the anchors: their text sits at the top.
    let bare = rewrite(&bytes, |n, x| if n.starts_with("ppt/slides/slide") { x.replace(r#" anchor="ctr""#, "") } else { x });
    let a = anchors(&deckcraft_pptx::import(&bare).expect("import"));
    assert!(a.iter().all(|x| *x == Some(Anchor::Top)), "{a:?}");

    // Also when the theme's shape defaults centre new shapes, as Office's themes do: PowerPoint
    // for Mac draws `<a:bodyPr/>` at the top under such an `spDef` (synthetic probe exported as PDF).
    let themed = rewrite(&bare, |n, x| {
        if n.starts_with("ppt/theme/") {
            x.replace(
                "<a:objectDefaults/>",
                r#"<a:objectDefaults><a:spDef><a:spPr/><a:bodyPr rtlCol="0" anchor="ctr"/><a:lstStyle/></a:spDef></a:objectDefaults>"#,
            )
        } else {
            x
        }
    });
    let a = anchors(&deckcraft_pptx::import(&themed).expect("import"));
    assert!(a.iter().all(|x| *x == Some(Anchor::Top)), "{a:?}");
}
