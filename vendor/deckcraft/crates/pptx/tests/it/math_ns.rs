//! Equations whose math namespace is declared above the equation: on the slide root or on a wrapper.

use std::io::{Cursor, Read, Write};

use deckcraft_geom::Xfrm;
use deckcraft_model::text::{Paragraph, Run, RunKind, RunProps, TextBody};
use deckcraft_model::{Shape, ShapeId, defaults};

const NS_M: &str = r#"xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math""#;

fn entries(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    (0..z.len())
        .map(|i| {
            let mut f = z.by_index(i).expect("entry");
            let mut d = vec![];
            f.read_to_end(&mut d).expect("read");
            (f.name().to_string(), d)
        })
        .collect()
}

fn rezip(es: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default();
    for (n, d) in es {
        z.start_file(n.as_str(), o).expect("start");
        z.write_all(d).expect("write");
    }
    z.finish().expect("finish").into_inner()
}

/// Export an equation, move its `xmlns:m` from the equation to the element `host` opens, and
/// return the OMML read back.
fn moved_to(host: &str) -> String {
    let mut p = defaults::new_presentation(None);
    let id = ShapeId(p.alloc_id().expect("id"));
    let omml = format!("<m:oMathPara {NS_M}><m:oMath><m:r><m:t>x=1</m:t></m:r></m:oMath></m:oMathPara>");
    let eq = Shape {
        id,
        name: "Equation".into(),
        xfrm: Some(Xfrm::new(10.0, 10.0, 200.0, 40.0)),
        text: Some(TextBody {
            paragraphs: vec![Paragraph {
                runs: vec![Run { text: "x=1".into(), props: RunProps::default(), kind: RunKind::Math { omml } }],
                ..Default::default()
            }],
            ..Default::default()
        }),
        ..Default::default()
    };
    if let Some(s) = p.slides.first_mut() {
        std::sync::Arc::make_mut(s).shapes = vec![eq];
    }
    let mut es = entries(&deckcraft_pptx::export(&p).expect("export"));
    let slide = es.iter_mut().find(|(n, _)| n == "ppt/slides/slide1.xml").expect("slide1");
    let xml = String::from_utf8(slide.1.clone()).expect("utf8");
    assert!(xml.contains(host), "{xml}");
    let xml = xml.replacen(&format!(" {NS_M}"), "", 1).replacen(host, &format!("{host}{NS_M} "), 1);
    assert_eq!(xml.matches(NS_M).count(), 1);
    slide.1 = xml.into_bytes();

    let q = deckcraft_pptx::import(&rezip(&es)).expect("import");
    let shape = q.slides[0].shapes.iter().find(|s| s.name == "Equation").expect("shape");
    let run = shape.text.as_ref().and_then(|t| t.paragraphs.first()).and_then(|p| p.runs.first()).expect("run");
    match &run.kind {
        RunKind::Math { omml } => omml.clone(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn math_namespace_declared_on_the_slide_root_is_kept() {
    let omml = moved_to("<p:sld ");
    assert!(omml.contains(NS_M), "{omml}");
}

#[test]
fn math_namespace_declared_on_an_enclosing_wrapper_is_kept() {
    let omml = moved_to("<mc:Choice ");
    assert!(omml.contains(NS_M), "{omml}");
}
