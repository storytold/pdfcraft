//! `place_slide` sets the text of a shape inside a group whose child coordinates scale it the way
//! the slide draws it: at the shape's size on the slide, with font sizes and insets in points.

use super::{Fill, Point, place_slide};
use deckcraft_model::style::ColorRef;
use deckcraft_model::text::TextBody;
use deckcraft_model::{Presentation, Shape, ShapeId, ShapeKind, Xfrm};

/// A deck whose slide shows a shape at (100, 100), 400 × 200 points, with 24 pt black text: on its
/// own, or inside a group whose child space is `scale` times smaller than the group.
fn deck(scale: Option<f64>) -> Presentation {
    let mut body = TextBody::from_text("Placed where it is drawn");
    body.paragraphs[0].runs[0].props.size = Some(24.0);
    body.paragraphs[0].runs[0].props.fill = Some(Fill::solid(ColorRef::rgb(deckcraft_color::Rgba::BLACK)));
    let shape = match scale {
        None => Shape { id: ShapeId(701), xfrm: Some(Xfrm::new(100.0, 100.0, 400.0, 200.0)), text: Some(body), ..Default::default() },
        Some(k) => {
            let child = Shape { id: ShapeId(701), xfrm: Some(Xfrm::new(0.0, 0.0, 400.0 / k, 200.0 / k)), text: Some(body), ..Default::default() };
            Shape {
                id: ShapeId(700),
                xfrm: Some(Xfrm::new(100.0, 100.0, 400.0, 200.0)),
                kind: ShapeKind::Group { children: vec![child], child: Xfrm::new(0.0, 0.0, 400.0 / k, 200.0 / k) },
                ..Default::default()
            }
        }
    };
    let mut p = Presentation::default();
    std::sync::Arc::make_mut(&mut p.slides[0]).shapes = vec![shape];
    p
}

/// Each placed glyph: its id, its origin on the slide and its run's drawn size (the font size
/// times the scale of the text's transform).
fn glyphs_on_slide(p: &Presentation) -> Vec<(u32, Point, f64)> {
    let placed = place_slide(p, 0);
    let mut out = vec![];
    for t in &placed.texts {
        let k = t.transform.determinant().abs().sqrt();
        for g in &t.layout.runs {
            for &(gid, x, y) in &g.glyphs {
                out.push((gid, t.transform * Point::new(x, y), g.size * k));
            }
        }
    }
    out
}

#[test]
fn text_in_a_scaled_group_is_placed_as_drawn() {
    let plain = glyphs_on_slide(&deck(None));
    assert!(!plain.is_empty());
    assert!(plain.iter().all(|g| (g.2 - 24.0).abs() < 1e-6), "{plain:?}");
    for scale in [1.0, 1600.0, 4.0, 0.25] {
        let grouped = glyphs_on_slide(&deck(Some(scale)));
        assert_eq!(grouped.len(), plain.len(), "scale {scale}");
        for (a, b) in grouped.iter().zip(&plain) {
            assert_eq!(a.0, b.0, "scale {scale}");
            assert!((a.1 - b.1).hypot() < 0.01, "scale {scale}: glyph at {:?}, on its own at {:?}", a.1, b.1);
            assert!((a.2 - b.2).abs() < 0.01, "scale {scale}: drawn at {} pt, on its own at {} pt", a.2, b.2);
        }
    }
}
