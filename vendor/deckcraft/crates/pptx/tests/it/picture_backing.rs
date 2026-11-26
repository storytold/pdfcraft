//! A picture's fill is a backing behind its transparent image, independent of its blip fill.
use std::sync::Arc;

use deckcraft_color::Rgba;
use deckcraft_model::style::{Fill, PictureFill, PictureMode};
use deckcraft_model::{ColorRef, Presentation, Shape, ShapeId, ShapeKind, Xfrm, defaults};

fn deck(fill: Option<Fill>, object: bool) -> Presentation {
    let mut p = defaults::new_presentation(None);
    let image = image::RgbaImage::from_fn(8, 4, |x, _| if x < 4 { image::Rgba([0, 0, 0, 0]) } else { image::Rgba([0, 0, 128, 255]) });
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image).write_to(&mut png, image::ImageFormat::Png).expect("PNG");
    let media = p.add_media("transparent.png", "image/png", png.into_inner()).expect("media");
    let kind = if object {
        ShapeKind::Opaque { xml: String::new(), preview: Some(media), label: "synthetic preview".into(), parts: vec![] }
    } else {
        ShapeKind::Picture { fill: PictureFill { media, ..Default::default() } }
    };
    let pic = Shape { id: ShapeId(102), name: "Picture".into(), xfrm: Some(Xfrm::new(20.0, 10.0, 80.0, 40.0)), fill, kind, ..Default::default() };
    let group = Shape {
        id: ShapeId(101),
        name: "Group".into(),
        xfrm: Some(Xfrm::new(100.0, 150.0, 400.0, 180.0)),
        fill: Some(Fill::solid(ColorRef::rgb(Rgba::WHITE))),
        kind: ShapeKind::Group { children: vec![pic], child: Xfrm::new(0.0, 0.0, 200.0, 120.0) },
        ..Default::default()
    };
    Arc::make_mut(&mut p.slides[0]).shapes.push(group);
    p
}

fn picture(p: &Presentation) -> &Shape {
    let g = p.slides[0].shapes.iter().find(|s| s.name == "Group").expect("group");
    let ShapeKind::Group { children, child } = &g.kind else { panic!("group kind") };
    assert_eq!(*child, Xfrm::new(0.0, 0.0, 200.0, 120.0));
    assert_eq!(g.xfrm, Some(Xfrm::new(100.0, 150.0, 400.0, 180.0)));
    assert_eq!(g.fill, Some(Fill::solid(ColorRef::rgb(Rgba::WHITE))));
    children.iter().find(|s| s.name == "Picture").expect("picture")
}

fn check_saves(mut p: Presentation, fill: Option<Fill>) {
    let expected_media = p.media[0].data.clone();
    for _ in 0..2 {
        p = deckcraft_pptx::import(&deckcraft_pptx::export(&p).expect("export")).expect("import");
        let pic = picture(&p);
        assert_eq!(pic.fill, fill, "keep the backing separate from the picture's blip fill");
        assert_eq!(pic.xfrm, Some(Xfrm::new(20.0, 10.0, 80.0, 40.0)));
        let ShapeKind::Picture { fill: image } = &pic.kind else { panic!("picture kind") };
        assert!(matches!(image.mode, PictureMode::Stretch { .. }));
        let media = p.media.iter().find(|m| m.id == image.media).expect("referenced image");
        assert_eq!(media.data, expected_media, "keep transparent PNG bytes unchanged");
    }
}

#[test]
fn a_transparent_picture_keeps_its_inherited_group_backing() {
    check_saves(deck(Some(Fill::Group), false), Some(Fill::Group));
}

#[test]
fn an_object_saved_as_its_picture_preview_keeps_its_inherited_group_backing() {
    // Empty kept XML deliberately exercises shape_with_id's preview fallback, not the ordinary
    // picture writer. Both must keep the same backing when they serialize p:pic.
    check_saves(deck(Some(Fill::Group), true), Some(Fill::Group));
}

#[test]
fn unfilled_and_directly_filled_pictures_keep_their_own_backing() {
    for fill in [None, Some(Fill::None), Some(Fill::solid(ColorRef::rgb(Rgba::rgb(200, 100, 20))))] {
        check_saves(deck(fill.clone(), false), fill);
    }
}
