//! "Auto-reverse" is written by PowerPoint on the behaviour time nodes, not on the effect node.

use std::io::{Cursor, Read, Write};

use deckcraft_geom::Xfrm;
use deckcraft_model::anim::{AnimClass, Animation};
use deckcraft_model::{Shape, ShapeId, defaults};

fn deck_with(effect: &str, behaviour: &str) -> Vec<bool> {
    let mut p = defaults::new_presentation(None);
    let id = ShapeId(p.alloc_id().expect("id"));
    let shape = Shape { id, name: "Box".into(), xfrm: Some(Xfrm::new(10.0, 10.0, 200.0, 100.0)), ..Default::default() };
    let sl = std::sync::Arc::make_mut(&mut p.slides[0]);
    sl.shapes = vec![shape];
    sl.animations.push(Animation { shape: id, class: AnimClass::Emphasis, effect: effect.into(), duration_ms: 1000, ..Default::default() });
    let mut z = zip::ZipArchive::new(Cursor::new(deckcraft_pptx::export(&p).expect("export"))).expect("zip");
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for i in 0..z.len() {
        let mut f = z.by_index(i).expect("entry");
        let name = f.name().to_string();
        let mut d = vec![];
        f.read_to_end(&mut d).expect("read");
        if name == "ppt/slides/slide1.xml" {
            let xml = String::from_utf8(d).expect("utf8");
            let at = xml.find(behaviour).expect("behaviour written");
            let cb = at + xml[at..].find("<p:cTn ").expect("behaviour node");
            d = format!("{}<p:cTn autoRev=\"1\" {}", &xml[..cb], &xml[cb + "<p:cTn ".len()..]).into_bytes();
        }
        out.start_file(name, zip::write::SimpleFileOptions::default()).expect("start");
        out.write_all(&d).expect("write");
    }
    let q = deckcraft_pptx::import(&out.finish().expect("finish").into_inner()).expect("import");
    q.slides[0].animations.iter().map(|a| a.auto_reverse).collect()
}

#[test]
fn grow_shrink_reads_auto_reverse_from_its_behaviour_node() {
    assert_eq!(deck_with("growShrink", "<p:animScale>"), [true]);
}

#[test]
fn color_pulse_behaviour_auto_reverse_is_not_the_option() {
    assert_eq!(deck_with("colorPulse", "<p:animClr "), [false]);
}
