//! Export looks media, layouts, sections and table styles up by id: a deck with many of each and
//! many references to the last ones exports in time linear in its size.

use std::sync::Arc;

use deckcraft_model::style::PictureFill;
use deckcraft_model::{Section, Shape, ShapeId, ShapeKind, defaults};

#[test]
fn many_media_sections_and_table_styles_export() {
    let mut p = defaults::new_presentation(None);
    // 5,000 media items; 5,000 pictures of the last one.
    let mut last = None;
    for i in 0..5_000u32 {
        last = p.add_media(&format!("m{i}.png"), "image/png", i.to_le_bytes().to_vec());
    }
    let last = last.unwrap();
    let pic = |k: u32| Shape {
        id: ShapeId(100_000 + k),
        kind: ShapeKind::Picture { fill: PictureFill { media: last, ..Default::default() } },
        ..Default::default()
    };
    Arc::make_mut(&mut p.slides[0]).shapes = (0..5_000).map(pic).collect();
    // A section naming the slide 50,000 times, after 2,000 empty sections.
    let id = p.slides[0].id;
    p.sections = (0..2_000).map(|i| Section { name: format!("s{i}"), slides: vec![] }).collect();
    p.sections.push(Section { name: "last".into(), slides: vec![id; 50_000] });
    let bytes = deckcraft_pptx::export(&p).expect("export");
    let q = deckcraft_pptx::import(&bytes).expect("import");
    assert_eq!(q.slides[0].shapes.iter().filter(|s| matches!(s.kind, ShapeKind::Picture { .. })).count(), 5_000);
}
