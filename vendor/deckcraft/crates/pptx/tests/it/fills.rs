//! Fills read from decks written by other tools.

use std::io::{Cursor, Read, Write};

use deckcraft_geom::Xfrm;
use deckcraft_model::style::{Fill, PictureFill, PictureMode};
use deckcraft_model::{Shape, ShapeId, defaults};

/// `bytes` with `edit` applied to every slide part.
fn edit_slides(bytes: &[u8], edit: impl Fn(String) -> String) -> Vec<u8> {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default();
    for i in 0..z.len() {
        let mut f = z.by_index(i).expect("entry");
        let mut d = vec![];
        f.read_to_end(&mut d).expect("read");
        let name = f.name().to_string();
        if name.starts_with("ppt/slides/slide") && name.ends_with(".xml") {
            d = edit(String::from_utf8(d).expect("utf8")).into_bytes();
        }
        out.start_file(name, o).expect("start");
        out.write_all(&d).expect("write");
    }
    out.finish().expect("finish").into_inner()
}

fn png() -> Vec<u8> {
    let mut out = Cursor::new(vec![]);
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([10, 200, 30])))
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("png");
    out.into_inner()
}

/// A deck with one shape filled with a stretched picture, exported.
fn deck() -> Vec<u8> {
    let mut p = defaults::new_presentation(None);
    let media = p.add_media("tile.png", "image/png", png()).expect("media");
    let id = ShapeId(p.alloc_id().expect("ids"));
    let first = p.slides[0].id;
    p.slide_mut(first).expect("slide").shapes.push(Shape {
        id,
        name: "Textured".into(),
        xfrm: Some(Xfrm::new(10.0, 10.0, 200.0, 100.0)),
        fill: Some(Fill::Picture(PictureFill { media, ..Default::default() })),
        ..Default::default()
    });
    deckcraft_pptx::export(&p).expect("export")
}

fn textured_mode(bytes: &[u8]) -> PictureMode {
    let q = deckcraft_pptx::import(bytes).expect("import");
    let s = q.slides[0].shapes.iter().find(|s| s.name == "Textured").expect("shape");
    match &s.fill {
        Some(Fill::Picture(pf)) => pf.mode.clone(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_shape_picture_fill_without_stretch_or_tile_tiles() {
    // tdf#128596: `<a:blipFill><a:blip/></a:blipFill>` on a shape, as some tools write it;
    // PowerPoint tiles the picture at its own size.
    let bare = edit_slides(&deck(), |s| {
        let s = regex_free_strip(&s, "<a:stretch>", "</a:stretch>");
        assert!(!s.contains("<a:stretch"), "the test deck lost its stretch");
        s
    });
    assert!(matches!(textured_mode(&bare), PictureMode::Tile { sx, sy, .. } if sx == 1.0 && sy == 1.0));
    // An explicit stretch is still a stretch.
    assert!(matches!(textured_mode(&deck()), PictureMode::Stretch { .. }));
}

/// `s` without the first `open`…`close` span inside a `blipFill`.
fn regex_free_strip(s: &str, open: &str, close: &str) -> String {
    let Some(b) = s.find("<a:blipFill") else { return s.to_string() };
    let Some(i) = s[b..].find(open).map(|i| i + b) else { return s.to_string() };
    let Some(j) = s[i..].find(close).map(|j| j + i + close.len()) else { return s.to_string() };
    format!("{}{}", &s[..i], &s[j..])
}

#[test]
fn a_gradient_without_stops_is_saved_without_stops() {
    // `a:gradFill` with no `a:gsLst` takes its stops from the style's fill in PowerPoint; saving
    // must not turn it into a gradient of its own (it used to become solid white).
    use deckcraft_model::style::{Gradient, GradientShape};
    let mut p = defaults::new_presentation(None);
    let id = ShapeId(p.alloc_id().expect("ids"));
    let first = p.slides[0].id;
    let own =
        Gradient { tile: [0.0; 4], stops: vec![], shape: GradientShape::Linear { angle: 45.0, scaled: true }, rotate_with_shape: true, flip: None };
    p.slide_mut(first).expect("slide").shapes.push(Shape {
        id,
        name: "Styled".into(),
        xfrm: Some(Xfrm::new(10.0, 10.0, 200.0, 100.0)),
        fill: Some(Fill::Gradient(own.clone())),
        style: Some(deckcraft_model::ShapeStyle::accent(deckcraft_color::SchemeSlot::Accent1)),
        ..Default::default()
    });
    let bytes = deckcraft_pptx::export(&p).expect("export");
    let q = deckcraft_pptx::import(&bytes).expect("import");
    let s = q.slides[0].shapes.iter().find(|s| s.name == "Styled").expect("shape");
    assert_eq!(s.fill, Some(Fill::Gradient(own)));
}

#[test]
fn a_fill_overlay_round_trips() {
    use deckcraft_model::style::{FillOverlay, Gradient, GradientShape, GradientStop};
    let mut p = defaults::new_presentation(None);
    let id = ShapeId(p.alloc_id().expect("ids"));
    let first = p.slides[0].id;
    let overlay = FillOverlay {
        blend: "mult".into(),
        fill: Fill::Gradient(Gradient {
            tile: [0.0; 4],
            stops: vec![
                GradientStop { pos: 0.0, color: deckcraft_model::ColorRef::scheme(deckcraft_color::SchemeSlot::Accent1) },
                GradientStop { pos: 1.0, color: deckcraft_model::ColorRef::rgb(deckcraft_color::Rgba::rgb(10, 20, 30)) },
            ],
            shape: GradientShape::Linear { angle: 4.5, scaled: true },
            rotate_with_shape: true,
            flip: None,
        }),
    };
    p.slide_mut(first).expect("slide").shapes.push(Shape {
        id,
        name: "Overlaid".into(),
        xfrm: Some(Xfrm::new(10.0, 10.0, 200.0, 100.0)),
        fill: Some(Fill::solid(deckcraft_model::ColorRef::rgb(deckcraft_color::Rgba::WHITE))),
        effects: Some(deckcraft_model::Effects { fill_overlay: Some(overlay.clone()), ..Default::default() }),
        ..Default::default()
    });
    let bytes = deckcraft_pptx::export(&p).expect("export");
    let q = deckcraft_pptx::import(&bytes).expect("import");
    let s = q.slides[0].shapes.iter().find(|s| s.name == "Overlaid").expect("shape");
    assert_eq!(s.effects.as_ref().and_then(|e| e.fill_overlay.clone()), Some(overlay));
}

#[test]
fn a_gradient_tile_rect_is_read_and_round_trips() {
    use deckcraft_model::style::{Gradient, GradientShape, GradientStop};
    // PowerPoint's "from top-left corner" radial: focus on the corner, tile reaching a box up and
    // to the left so it centres on that corner.
    let g = Gradient {
        stops: vec![
            GradientStop { pos: 0.0, color: deckcraft_model::ColorRef::rgb(deckcraft_color::Rgba::rgb(0, 128, 255)) },
            GradientStop { pos: 1.0, color: deckcraft_model::ColorRef::rgb(deckcraft_color::Rgba::WHITE) },
        ],
        shape: GradientShape::Path { path: "circle".into(), focus: [0.0, 0.0, 1.0, 1.0] },
        rotate_with_shape: true,
        tile: [-1.0, -1.0, 0.0, 0.0],
        flip: None,
    };
    let mut p = defaults::new_presentation(None);
    let id = ShapeId(p.alloc_id().expect("ids"));
    let first = p.slides[0].id;
    p.slide_mut(first).expect("slide").shapes.push(Shape {
        id,
        name: "Corner".into(),
        xfrm: Some(Xfrm::new(10.0, 10.0, 200.0, 100.0)),
        fill: Some(Fill::Gradient(g.clone())),
        ..Default::default()
    });
    let bytes = deckcraft_pptx::export(&p).expect("export");
    let q = deckcraft_pptx::import(&bytes).expect("import");
    let s = q.slides[0].shapes.iter().find(|s| s.name == "Corner").expect("shape");
    assert_eq!(s.fill, Some(Fill::Gradient(g)));
    // As written by PowerPoint (attributes it leaves out are 0).
    let edited =
        edit_slides(&bytes, |x| x.replace("<a:tileRect l=\"-100000\" t=\"-100000\" r=\"0\" b=\"0\"/>", "<a:tileRect l=\"-100000\" t=\"-100000\"/>"));
    let q = deckcraft_pptx::import(&edited).expect("import");
    let s = q.slides[0].shapes.iter().find(|s| s.name == "Corner").expect("shape");
    assert!(matches!(&s.fill, Some(Fill::Gradient(g)) if g.tile == [-1.0, -1.0, 0.0, 0.0]), "{:?}", s.fill);
}

#[test]
fn a_table_naming_no_style_is_drawn_with_the_table_grid() {
    let mut p = defaults::new_presentation(None);
    let id = ShapeId(p.alloc_id().expect("ids"));
    let first = p.slides[0].id;
    let mut t = deckcraft_model::Table::new(2, 2, 200.0, 40.0);
    t.style = "none".into();
    p.slide_mut(first).expect("slide").shapes.push(Shape {
        id,
        name: "Grid".into(),
        xfrm: Some(Xfrm::new(10.0, 10.0, 200.0, 80.0)),
        kind: deckcraft_model::ShapeKind::Table(t),
        ..Default::default()
    });
    let bytes = deckcraft_pptx::export(&p).expect("export");
    let style = |bytes: &[u8]| {
        let q = deckcraft_pptx::import(bytes).expect("import");
        let s = q.slides[0].shapes.iter().find(|s| s.name == "Grid").expect("shape").clone();
        match s.kind {
            deckcraft_model::ShapeKind::Table(t) => t.style,
            other => panic!("{other:?}"),
        }
    };
    // Written with its style, it reads back as written.
    assert_eq!(style(&bytes), "none");
    // Without a:tableStyleId PowerPoint draws thin dark grid lines.
    let bare = edit_slides(&bytes, |x| {
        let (Some(a), Some(b)) = (x.find("<a:tableStyleId>"), x.find("</a:tableStyleId>")) else { return x };
        format!("{}{}", &x[..a], &x[b + "</a:tableStyleId>".len()..])
    });
    assert_eq!(style(&bare), "grid");
}

#[test]
fn black_and_white_and_inverted_alpha_pictures_round_trip() {
    let mut p = defaults::new_presentation(None);
    let media = p.add_media("tile.png", "image/png", png()).expect("media");
    let id = ShapeId(p.alloc_id().expect("ids"));
    let first = p.slides[0].id;
    let mut pf = PictureFill { media, ..Default::default() };
    pf.adjust.bilevel = Some(0.3);
    pf.adjust.alpha_inv = true;
    p.slide_mut(first).expect("slide").shapes.push(Shape {
        id,
        name: "BW".into(),
        xfrm: Some(Xfrm::new(10.0, 10.0, 200.0, 100.0)),
        fill: Some(Fill::Picture(pf)),
        ..Default::default()
    });
    let bytes = deckcraft_pptx::export(&p).expect("export");
    let q = deckcraft_pptx::import(&bytes).expect("import");
    let s = q.slides[0].shapes.iter().find(|s| s.name == "BW").expect("shape");
    match &s.fill {
        Some(Fill::Picture(pf)) => assert_eq!((pf.adjust.bilevel, pf.adjust.alpha_inv), (Some(0.3), true)),
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_upright_picture_fill_round_trips() {
    let bytes = deck();
    let edited = edit_slides(&bytes, |x| x.replacen("rotWithShape=\"1\"", "rotWithShape=\"0\"", 1));
    let q = deckcraft_pptx::import(&edited).expect("import");
    let s = q.slides[0].shapes.iter().find(|s| s.name == "Textured").expect("shape");
    let Some(Fill::Picture(pf)) = &s.fill else { panic!("{:?}", s.fill) };
    assert!(pf.upright);
    let again = deckcraft_pptx::export(&q).expect("export");
    let r = deckcraft_pptx::import(&again).expect("import");
    let s = r.slides[0].shapes.iter().find(|s| s.name == "Textured").expect("shape");
    assert!(matches!(&s.fill, Some(Fill::Picture(pf)) if pf.upright));
    // Rotating with the shape is the default.
    assert!(matches!(textured_fill(&bytes), Some(Fill::Picture(pf)) if !pf.upright));
}

fn textured_fill(bytes: &[u8]) -> Option<Fill> {
    let q = deckcraft_pptx::import(bytes).expect("import");
    q.slides[0].shapes.iter().find(|s| s.name == "Textured").and_then(|s| s.fill.clone())
}
