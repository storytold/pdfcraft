//! Synthetic interactive sequences must keep one step per click across a save.

use std::io::{Cursor, Read};
use std::sync::Arc;

use deckcraft_geom::Xfrm;
use deckcraft_model::anim::{AnimStart, Animation, TextBuild};
use deckcraft_model::text::TextBody;
use deckcraft_model::{Presentation, Shape, ShapeId, defaults};

fn deck() -> Presentation {
    let mut p = defaults::new_presentation(None);
    let shapes: Vec<_> = ["Button", "Other button", "Content"]
        .into_iter()
        .map(|name| Shape {
            id: ShapeId(p.alloc_id().expect("id")),
            name: name.into(),
            xfrm: Some(Xfrm::new(40.0, 40.0, 200.0, 100.0)),
            text: Some(TextBody::from_text("First\nSecond\nThird")),
            ..Default::default()
        })
        .collect();
    Arc::make_mut(&mut p.slides[0]).shapes = shapes;
    p
}

fn round(p: &Presentation) -> Presentation {
    deckcraft_pptx::import(&deckcraft_pptx::export(p).expect("export")).expect("import")
}

fn slide_xml(p: &Presentation) -> String {
    let bytes = deckcraft_pptx::export(p).expect("export");
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    let mut xml = String::new();
    z.by_name("ppt/slides/slide1.xml").expect("slide").read_to_string(&mut xml).expect("xml");
    xml
}

#[test]
fn triggered_click_steps_survive_import_edit_save_reopen() {
    use AnimStart::{AfterPrevious, OnClick, WithPrevious};
    let mut p = deck();
    let ids: Vec<_> = p.slides[0].shapes.iter().map(|s| s.id).collect();
    let starts = [OnClick, WithPrevious, AfterPrevious, OnClick, WithPrevious, OnClick];
    for (i, start) in starts.into_iter().enumerate() {
        Arc::make_mut(&mut p.slides[0]).animations.push(Animation {
            shape: ids[2],
            trigger: Some(ids[0]),
            start,
            effect: "appear".into(),
            delay_ms: i as u32 * 100,
            ..Default::default()
        });
    }
    // An independent button and the main sequence must not borrow this button's steps.
    for trigger in [Some(ids[1]), None] {
        for start in [OnClick, AfterPrevious, OnClick] {
            Arc::make_mut(&mut p.slides[0]).animations.push(Animation { shape: ids[2], trigger, start, ..Default::default() });
        }
    }
    let mut q = round(&p);
    for pass in 0..2 {
        for trigger in [Some(ids[0]), Some(ids[1]), None] {
            let state = |deck: &Presentation| {
                deck.slides[0].animations.iter().filter(|a| a.trigger == trigger).map(|a| (a.start, a.delay_ms)).collect::<Vec<_>>()
            };
            assert_eq!(state(&q), state(&p), "trigger {trigger:?}, pass {pass}");
        }
        assert_eq!(q.slides[0].animations.len(), p.slides[0].animations.len());
        Arc::make_mut(&mut q.slides[0]).shapes[2].name = "Edited content".into();
        q = round(&q);
    }
    let xml = slide_xml(&p);
    assert_eq!(xml.matches("nodeType=\"interactiveSeq\"").count(), 2);
    assert_eq!(xml.matches("nodeType=\"clickEffect\"").count(), 7);
    // The first step in each interactive sequence begins at zero; subsequent steps wait.
    assert_eq!(xml.matches("delay=\"indefinite\"").count(), 5);
}

#[test]
fn triggered_paragraph_build_keeps_a_click_for_each_paragraph() {
    let mut p = deck();
    let ids: Vec<_> = p.slides[0].shapes.iter().map(|s| s.id).collect();
    Arc::make_mut(&mut p.slides[0]).animations =
        vec![Animation { shape: ids[2], trigger: Some(ids[0]), text_build: TextBuild::ByParagraph, start: AnimStart::OnClick, ..Default::default() }];
    for q in [p.clone(), round(&p)] {
        let xml = slide_xml(&q);
        assert_eq!(xml.matches("nodeType=\"clickEffect\"").count(), 3);
        assert_eq!(xml.matches("delay=\"indefinite\"").count(), 2);
        let back = round(&q);
        assert_eq!(back.slides[0].animations.len(), 1);
        assert_eq!(back.slides[0].animations[0].text_build, TextBuild::ByParagraph);
        assert_eq!(back.slides[0].animations[0].trigger, Some(ids[0]));
    }
}
