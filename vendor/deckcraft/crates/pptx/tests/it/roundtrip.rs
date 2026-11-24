//! Round trips of decks built in code: model → PPTX → model.

use std::sync::Arc;

use deckcraft_color::{ColorTransform, Rgba, SchemeSlot};
use deckcraft_geom::Xfrm;
use deckcraft_model::anim::{AnimClass, AnimStart, Animation, TextBuild, Transition};
use deckcraft_model::chart::{Chart, ChartType};
use deckcraft_model::style::{
    ColorRef, Dash, Effects, Fill, Glow, Gradient, GradientFlip, GradientShape, GradientStop, Line, LineEnd, PatternFill, PictureAdjust, PictureFill,
    PictureMode, Shadow,
};
use deckcraft_model::table::Table;
use deckcraft_model::text::{
    Action, Align, Anchor, AutoFit, BodyProps, Bullet, Caps, Hyperlink, Paragraph, Run, RunKind, RunProps, Spacing, Strike, TextBody, TextOverflow,
};
use deckcraft_model::{
    Background, Comment, CustomPath, CustomShow, Geom, InkStroke, MediaClip, Presentation, Section, Shape, ShapeId, ShapeKind, Slide, SlideId,
    defaults,
};

fn png(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(w, h, image::Rgb(rgb));
    let mut out = std::io::Cursor::new(vec![]);
    image::DynamicImage::ImageRgb8(img).write_to(&mut out, image::ImageFormat::Png).expect("png");
    out.into_inner()
}

fn shape(p: &mut Presentation, name: &str, x: f64, y: f64, w: f64, h: f64) -> Shape {
    Shape { id: ShapeId(p.alloc_id().expect("ids")), name: name.into(), xfrm: Some(Xfrm::new(x, y, w, h)), ..Default::default() }
}

fn new_slide(p: &mut Presentation, title: &str) -> Slide {
    let layout = defaults::layout_of_kind(p, deckcraft_model::LayoutType::TitleOnly).unwrap_or_default();
    let mut s = defaults::new_slide(p, layout).expect("slide");
    if let Some(t) = s.shapes.first_mut() {
        t.text = Some(TextBody::from_text(title));
    }
    s
}

/// A deck exercising most of what the model can hold.
pub fn rich_deck() -> Presentation {
    let mut p = defaults::new_presentation(None);
    p.props.title = "Rich deck".into();
    p.props.author = "DeckCraft tests".into();
    p.props.created = "2025-01-02T03:04:05Z".into();
    let img = p.add_media("image1.png", "image/png", png(40, 30, [200, 40, 40])).expect("media");
    let img2 = p.add_media("image2.png", "image/png", png(8, 8, [20, 140, 220])).expect("media");
    let video = p.add_media("clip.mp4", "video/mp4", b"\x00\x00\x00\x18ftypmp42not-a-real-video".to_vec()).expect("media");

    // Slide 1: title slide with notes and formatted text.
    {
        let first = p.slides[0].id;
        let s = p.slide_mut(first).expect("slide");
        s.notes = TextBody::from_text("Speaker notes line 1\nline 2");
        let t = &mut s.shapes[0];
        t.text = Some(TextBody {
            paragraphs: vec![Paragraph {
                runs: vec![
                    Run::with("Bold ", RunProps { bold: Some(true), size: Some(40.0), ..Default::default() }),
                    Run::with(
                        "red italic",
                        RunProps {
                            italic: Some(true),
                            fill: Some(Fill::solid(ColorRef::rgb(Rgba::rgb(255, 0, 0)))),
                            underline: Some("sng".into()),
                            strike: Some(Strike::Single),
                            caps: Some(Caps::Small),
                            spacing: Some(1.5),
                            baseline: Some(0.3),
                            font: Some("Lato".into()),
                            lang: Some("en-GB".into()),
                            ..Default::default()
                        },
                    ),
                    Run { text: String::new(), props: RunProps::default(), kind: RunKind::Break },
                    Run::with(
                        "link",
                        RunProps {
                            link: Some(Hyperlink {
                                action: Action::Url { url: "https://example.org/a?b=1&c=2".into() },
                                tooltip: "tip".into(),
                                highlight_click: false,
                                sound: None,
                                stop_sound: false,
                                zoom_return: None,
                            }),
                            ..Default::default()
                        },
                    ),
                ],
                ..Default::default()
            }],
            ..Default::default()
        });
        s.transition = Some(Transition {
            kind: "fade".into(),
            option: "throughBlack".into(),
            duration_ms: 700,
            advance_after_ms: Some(3000),
            ..Default::default()
        });
    }

    // Slide 2: shapes, fills, lines, effects, custom geometry, group, connector.
    let mut s = new_slide(&mut p, "Shapes");
    let mut a = shape(&mut p, "Gradient Star", 100.0, 120.0, 150.0, 150.0);
    a.geom = Geom::Preset { name: "star5".into(), adj: vec![20000.0] };
    a.fill = Some(Fill::Gradient(Gradient {
        tile: [0.0; 4],
        stops: vec![
            GradientStop { pos: 0.0, color: ColorRef::scheme(SchemeSlot::Accent1) },
            GradientStop { pos: 1.0, color: ColorRef::scheme(SchemeSlot::Accent2).with(ColorTransform::LumMod(75000)) },
        ],
        shape: GradientShape::Linear { angle: 45.0, scaled: false },
        rotate_with_shape: true,

        flip: Some(GradientFlip::Xy),
    }));
    a.line = Some(Line { width: Some(3.0), dash: Some(Dash::Preset { name: "dash".into() }), ..Line::solid(ColorRef::rgb(Rgba::rgb(0, 0, 0)), 3.0) });
    a.effects = Some(Effects {
        outer_shadow: Some(Shadow {
            color: ColorRef::rgb(Rgba::BLACK).with(ColorTransform::Alpha(40000)),
            blur: 6.0,
            dist: 3.0,
            dir: 90.0,
            inner: false,
            sx: 1.0,
            sy: 1.0,
            kx: 0.0,
            ky: 0.0,
            align: "b".into(),
            rotate_with_shape: false,
        }),
        glow: Some(Glow { color: ColorRef::scheme(SchemeSlot::Accent6), radius: 8.0 }),
        soft_edge: Some(2.0),
        ..Default::default()
    });
    a.xfrm = Some(Xfrm { rot: 30.0, flip_h: true, ..Xfrm::new(100.0, 120.0, 150.0, 150.0) });
    let mut b = shape(&mut p, "Pattern Box", 300.0, 120.0, 120.0, 80.0);
    b.geom = Geom::Preset { name: "roundRect".into(), adj: vec![30000.0] };
    b.fill =
        Some(Fill::Pattern(PatternFill { preset: "dkDnDiag".into(), fg: ColorRef::scheme(SchemeSlot::Accent3), bg: ColorRef::rgb(Rgba::WHITE) }));
    b.text = Some(TextBody {
        body: BodyProps { anchor: Some(Anchor::Bottom), inset_l: Some(10.0), wrap: Some(false), autofit: Some(AutoFit::Shape), ..Default::default() },
        paragraphs: vec![Paragraph {
            props: deckcraft_model::text::ParaProps {
                align: Some(Align::Right),
                line_spacing: Some(Spacing::Pct(1.5)),
                space_before: Some(Spacing::Pts(6.0)),
                bullet: Some(Bullet::AutoNum { scheme: "romanUcPeriod".into(), start_at: 3 }),
                ..Default::default()
            },
            runs: vec![Run::new("numbered")],
            level: 2,
            ..Default::default()
        }],
        ..Default::default()
    });
    b.click = Some(Hyperlink {
        action: Action::NextSlide,
        tooltip: String::new(),
        highlight_click: true,
        sound: None,
        stop_sound: false,
        zoom_return: None,
    });
    let mut c = shape(&mut p, "Freeform", 450.0, 120.0, 100.0, 100.0);
    c.geom = Geom::Custom {
        paths: vec![CustomPath {
            w: 100000.0,
            h: 100000.0,
            d: "M 0 0 L 100000 0 L 50000 100000 Z".into(),
            fill: deckcraft_geom::preset::FillMode::Norm,
            stroke: true,
        }],
    };
    c.fill = Some(Fill::solid(ColorRef::scheme(SchemeSlot::Accent4)));
    let child1 = Shape { geom: Geom::preset("ellipse"), ..shape(&mut p, "Child 1", 0.0, 0.0, 50.0, 50.0) };
    let child2 = Shape { geom: Geom::preset("rect"), ..shape(&mut p, "Child 2", 60.0, 0.0, 50.0, 50.0) };
    let (c1, c2) = (child1.id, child2.id);
    let mut g = shape(&mut p, "Group", 600.0, 120.0, 220.0, 100.0);
    g.kind = ShapeKind::Group { children: vec![child1, child2], child: Xfrm::new(0.0, 0.0, 110.0, 50.0) };
    let mut cx = shape(&mut p, "Connector", 600.0, 300.0, 200.0, 0.0);
    cx.kind = ShapeKind::Connector { start: Some((c1, 2)), end: Some((c2, 0)) };
    cx.geom = Geom::preset("straightConnector1");
    cx.line = Some(Line {
        tail: Some(LineEnd { kind: "triangle".into(), w: "med".into(), len: "lg".into() }),
        ..Line::solid(ColorRef::scheme(SchemeSlot::Tx1), 2.0)
    });
    let mut tb = shape(&mut p, "Text Box", 100.0, 400.0, 300.0, 40.0);
    tb.text_box = true;
    tb.text = Some(TextBody::from_text("A text box"));
    tb.descr = "alt text".into();
    let mut bgs = shape(&mut p, "Uses background", 450.0, 400.0, 60.0, 60.0);
    bgs.fill = Some(Fill::Background);
    s.shapes.extend([a, b, c, g, cx, tb, bgs]);
    s.background = Some(Background::Fill { fill: Fill::solid(ColorRef::rgb(Rgba::rgb(250, 248, 240))) });
    s.transition = Some(Transition { kind: "morph".into(), option: "words".into(), duration_ms: 2000, ..Default::default() });
    let ids: Vec<ShapeId> = s.shapes.iter().map(|x| x.id).collect();
    s.animations = vec![
        Animation { shape: ids[1], effect: "fly".into(), option: "l".into(), duration_ms: 750, ..Default::default() },
        Animation { shape: ids[2], effect: "wipe".into(), option: "t".into(), start: AnimStart::WithPrevious, delay_ms: 250, ..Default::default() },
        Animation {
            shape: ids[3],
            class: AnimClass::Emphasis,
            effect: "spin".into(),
            start: AnimStart::AfterPrevious,
            duration_ms: 2000,
            ..Default::default()
        },
        Animation { shape: ids[4], class: AnimClass::Exit, effect: "fadeOut".into(), duration_ms: 400, ..Default::default() },
        Animation {
            shape: ids[2],
            class: AnimClass::Path,
            effect: "lines".into(),
            option: "right".into(),
            path: Some("M 0 0 L 0.25 0 E".into()),
            duration_ms: 2000,
            ..Default::default()
        },
        Animation { shape: ids[6], effect: "zoom".into(), option: "in".into(), trigger: Some(ids[3]), duration_ms: 500, ..Default::default() },
    ];
    s.comments = vec![Comment {
        author: "Ada".into(),
        initials: "AL".into(),
        text: "Nice star".into(),
        date: "2025-02-03T04:05:06.000".into(),
        x: 120.0,
        y: 64.0,
        ..Default::default()
    }];
    p.slides.push(Arc::new(s));

    // Slide 3: pictures, media, ink.
    let mut s = new_slide(&mut p, "Pictures");
    let mut pic = shape(&mut p, "Picture", 100.0, 120.0, 200.0, 150.0);
    pic.kind = ShapeKind::Picture {
        fill: PictureFill {
            media: img,
            crop: [0.1, 0.05, 0.2, 0.0],
            mode: PictureMode::Stretch { fill_rect: [0.0; 4] },
            alpha: Some(0.5),
            adjust: PictureAdjust { brightness: 0.2, contrast: -0.1, grayscale: true, ..Default::default() },
            upright: false,
            artistic: None,
        },
    };
    let mut tiled = shape(&mut p, "Tiled", 350.0, 120.0, 200.0, 150.0);
    tiled.fill = Some(Fill::Picture(PictureFill {
        media: img2,
        mode: PictureMode::Tile { tx: 0.0, ty: 0.0, sx: 0.5, sy: 0.5, flip: "none".into(), align: "tl".into() },
        ..Default::default()
    }));
    let mut vid = shape(&mut p, "Video", 600.0, 120.0, 240.0, 135.0);
    vid.kind = ShapeKind::Media(MediaClip {
        media: video,
        video: true,
        poster: Some(img),
        crop: [0.1, 0.0, 0.2, 0.25],
        volume: 1.0,
        trim_start_ms: 500,
        fade_in_ms: 250,
        ..Default::default()
    });
    let mut ink = shape(&mut p, "Ink", 100.0, 350.0, 200.0, 100.0);
    ink.kind = ShapeKind::Ink {
        strokes: vec![InkStroke {
            points: vec![(100.0, 350.0, 0.5), (150.0, 400.0, 0.5), (300.0, 360.0, 0.5)],
            color: Rgba::rgb(0, 0, 255),
            width: 3.0,
            highlighter: false,
        }],
    };
    s.shapes.extend([pic, tiled, vid, ink]);
    s.transition = Some(Transition { kind: "vortex".into(), option: "r".into(), duration_ms: 3000, ..Default::default() });
    s.hidden = true;
    p.slides.push(Arc::new(s));

    // Slide 4: table with merges and a chart per type.
    let mut s = new_slide(&mut p, "Table");
    let mut t = Table::new(3, 3, 450.0, 30.0);
    for r in 0..3 {
        for c in 0..3 {
            if let Some(cell) = t.cell_mut(r, c) {
                cell.text = TextBody::from_text(&format!("r{r}c{c}"));
            }
        }
    }
    t.merge(1, 0, 2, 1);
    t.style = "light2-accent3".into();
    if let Some(cell) = t.cell_mut(0, 2) {
        cell.fill = Some(Fill::solid(ColorRef::rgb(Rgba::rgb(255, 255, 0))));
        cell.margins = Some([5.0, 5.0, 2.0, 2.0]);
        cell.anchor = Some(Anchor::Middle);
        cell.borders[3] = Some(Line::solid(ColorRef::rgb(Rgba::BLACK), 2.0));
    }
    let mut ts = shape(&mut p, "Table 1", 100.0, 120.0, 450.0, 90.0);
    ts.kind = ShapeKind::Table(t);
    s.shapes.push(ts);
    s.transition = Some(Transition {
        kind: "split".into(),
        option: "vertIn".into(),
        duration_ms: 1500,
        advance_on_click: false,
        advance_after_ms: Some(2000),
        ..Default::default()
    });
    p.slides.push(Arc::new(s));

    let kinds = [
        ChartType::Column,
        ChartType::StackedBar,
        ChartType::PercentColumn,
        ChartType::Line,
        ChartType::LineMarkers,
        ChartType::Area,
        ChartType::Pie,
        ChartType::Doughnut,
        ChartType::Scatter,
        ChartType::Radar,
        ChartType::Bubble,
    ];
    let mut s = new_slide(&mut p, "Charts");
    for (i, k) in kinds.iter().enumerate() {
        let mut ch = shape(&mut p, &format!("Chart {i}"), 20.0 + (i % 4) as f64 * 230.0, 100.0 + (i / 4) as f64 * 140.0, 220.0, 130.0);
        let mut chart = Chart::sample(*k);
        chart.title = Some(k.label().to_string());
        ch.kind = ShapeKind::Chart(Box::new(chart));
        s.shapes.push(ch);
    }
    let mut combo = Chart::sample(ChartType::Column);
    combo.kind = ChartType::Combo;
    combo.series[0].kind = Some(ChartType::Column);
    combo.series[1].kind = Some(ChartType::Line);
    combo.series[2].kind = Some(ChartType::Line);
    let mut ch = shape(&mut p, "Combo", 710.0, 380.0, 220.0, 130.0);
    ch.kind = ShapeKind::Chart(Box::new(combo));
    s.shapes.push(ch);
    p.slides.push(Arc::new(s));

    // Slide 6: bulleted content with a by-paragraph build, a slide link and an equation.
    let layout = defaults::layout_of_kind(&p, deckcraft_model::LayoutType::TitleAndContent).unwrap_or_default();
    let mut s = defaults::new_slide(&mut p, layout).expect("slide");
    s.shapes[0].text = Some(TextBody::from_text("Build"));
    s.shapes[1].text = Some(TextBody::from_text("One\nTwo\nThree"));
    let body = s.shapes[1].id;
    s.animations = vec![Animation { shape: body, effect: "fade".into(), text_build: TextBuild::ByParagraph, duration_ms: 500, ..Default::default() }];
    let target = p.slides[1].id;
    let mut link = shape(&mut p, "Go to shapes", 600.0, 450.0, 200.0, 40.0);
    link.text = Some(TextBody::from_text("Back to shapes"));
    link.click = Some(Hyperlink {
        action: Action::Slide { slide: target },
        tooltip: String::new(),
        highlight_click: false,
        sound: None,
        stop_sound: false,
        zoom_return: None,
    });
    let mut eq = shape(&mut p, "Equation", 100.0, 450.0, 300.0, 40.0);
    eq.text = Some(TextBody {
        paragraphs: vec![Paragraph {
            runs: vec![Run {
                text: "x=1".into(),
                props: RunProps::default(),
                kind: RunKind::Math {
                    omml: r#"<m:oMathPara xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math"><m:oMath><m:r><m:t>x=1</m:t></m:r></m:oMath></m:oMathPara>"#.into(),
                },
            }],
            ..Default::default()
        }],
        ..Default::default()
    });
    s.shapes.extend([link, eq]);
    s.transition = Some(Transition { kind: "curtains".into(), duration_ms: 2000, ..Default::default() });
    p.slides.push(Arc::new(s));

    let ids: Vec<SlideId> = p.slides.iter().map(|s| s.id).collect();
    p.sections = vec![Section { name: "Start".into(), slides: ids[..2].to_vec() }, Section { name: "Rest".into(), slides: ids[2..].to_vec() }];
    p.custom_shows = vec![CustomShow { name: "Short".into(), slides: vec![ids[0], ids[3]] }];
    p.first_slide_number = 0;
    p
}

fn round(p: &Presentation) -> Presentation {
    let bytes = deckcraft_pptx::export(p).expect("export");
    deckcraft_pptx::import(&bytes).expect("import")
}

fn by_name<'a>(s: &'a Slide, name: &str) -> &'a Shape {
    let mut found = None;
    deckcraft_model::walk(&s.shapes, &mut |x, _| {
        if found.is_none() && x.name == name {
            found = Some(x);
        }
    });
    found.unwrap_or_else(|| panic!("no shape {name}"))
}

#[test]
fn rich_deck_writes_to_dir() {
    let p = rich_deck();
    let bytes = deckcraft_pptx::export(&p).expect("export");
    if let Ok(dir) = std::env::var("DECKCRAFT_PPTX_OUT") {
        std::fs::write(format!("{dir}/rich.pptx"), &bytes).expect("write");
    }
    assert!(bytes.len() > 1000);
}

#[test]
fn deck_level_round_trip() {
    let p = rich_deck();
    let q = round(&p);
    assert_eq!(q.slides.len(), p.slides.len());
    assert!(q.validate().is_empty(), "{:?}", q.validate());
    assert_eq!(q.slide_size, p.slide_size);
    assert_eq!(q.first_slide_number, 0);
    assert_eq!(q.props.title, "Rich deck");
    assert_eq!(q.props.author, "DeckCraft tests");
    assert_eq!(q.props.created, "2025-01-02T03:04:05Z");
    assert_eq!(
        q.sections.iter().map(|s| (s.name.clone(), s.slides.len())).collect::<Vec<_>>(),
        vec![("Start".to_string(), 2), ("Rest".to_string(), 4)]
    );
    assert_eq!(q.custom_shows.len(), 1);
    assert_eq!(q.custom_shows[0].slides, vec![q.slides[0].id, q.slides[3].id]);
    assert!(q.slides[2].hidden);
    // Themes and layouts.
    assert_eq!(q.masters.len(), 1);
    let (pm, qm) = (&p.masters[0], &q.masters[0]);
    assert_eq!(qm.theme.colors.colors, pm.theme.colors.colors);
    assert_eq!(qm.theme.fonts.major.latin, pm.theme.fonts.major.latin);
    assert_eq!(qm.layouts.len(), pm.layouts.len());
    for (a, b) in pm.layouts.iter().zip(qm.layouts.iter()) {
        assert_eq!((a.name.as_str(), a.kind, a.shapes.len()), (b.name.as_str(), b.kind, b.shapes.len()));
    }
    assert_eq!(qm.title_style, pm.title_style);
    assert_eq!(qm.body_style, pm.body_style);
    assert_eq!(q.default_text_style, p.default_text_style);
    for (a, b) in p.slides.iter().zip(q.slides.iter()) {
        let (la, lb) = (p.layout(a.layout).map(|x| &x.1.name), q.layout(b.layout).map(|x| &x.1.name));
        assert_eq!(la, lb);
    }
}

#[test]
fn text_and_formatting_round_trip() {
    let p = rich_deck();
    let q = round(&p);
    assert_eq!(q.slides[0].notes_text(), "Speaker notes line 1\nline 2");
    let (a, b) = (&p.slides[0].shapes[0], &q.slides[0].shapes[0]);
    assert_eq!(a.text, b.text);
    assert!(b.xfrm.is_none(), "placeholder without xfrm inherits");
    let sa = by_name(&p.slides[1], "Pattern Box");
    let sb = by_name(&q.slides[1], "Pattern Box");
    assert_eq!(sa.text, sb.text);
    assert_eq!(sa.click, sb.click);
    assert!(by_name(&q.slides[1], "Text Box").text_box);
    assert_eq!(by_name(&q.slides[1], "Text Box").descr, "alt text");
    let la = by_name(&p.slides[5], "Go to shapes");
    let lb = by_name(&q.slides[5], "Go to shapes");
    assert_eq!(lb.click.as_ref().map(|h| h.action.clone()), Some(Action::Slide { slide: q.slides[1].id }));
    assert!(la.click.is_some());
    let eq = by_name(&q.slides[5], "Equation");
    let run = &eq.text.as_ref().expect("text").paragraphs[0].runs[0];
    assert!(matches!(&run.kind, RunKind::Math { omml } if omml.contains("oMath")), "{run:?}");
    assert_eq!(run.text, "x=1");
}

#[test]
fn centered_cell_anchor_survives_two_pptx_round_trips() {
    let mut p = Presentation::default();
    let mut slide = new_slide(&mut p, "Cell anchor");
    let mut table_shape = shape(&mut p, "Table", 10.0, 10.0, 200.0, 80.0);
    let mut table = Table::new(1, 1, 200.0, 80.0);
    table.rows[0].cells[0].anchor_ctr = Some(true);
    table_shape.kind = ShapeKind::Table(table);
    slide.shapes.push(table_shape);
    p.slides.push(Arc::new(slide));

    let once = round(&p);
    let twice = round(&once);
    let slide = twice.slides.last().expect("added slide");
    let ShapeKind::Table(table) = &by_name(slide, "Table").kind else { panic!("table") };
    assert_eq!(table.rows[0].cells[0].anchor_ctr, Some(true));
}

#[test]
fn normalized_character_height_survives_two_pptx_round_trips() {
    let mut p = Presentation::default();
    let mut slide = new_slide(&mut p, "Normalized height");
    let mut enabled = shape(&mut p, "Enabled", 10.0, 10.0, 200.0, 80.0);
    let mut enabled_body = TextBody::from_text("enabled");
    enabled_body.paragraphs[0].runs[0].props.normalize_height = Some(true);
    enabled_body.paragraphs[0].runs[0].props.size = Some(18.0);
    enabled_body.paragraphs[0].runs[0].props.caps = Some(Caps::Small);
    enabled_body.paragraphs[0].runs[0].props.spacing = Some(1.5);
    enabled.text = Some(enabled_body);
    let mut disabled = shape(&mut p, "Disabled", 10.0, 100.0, 200.0, 80.0);
    let mut disabled_body = TextBody::from_text("disabled");
    disabled_body.paragraphs[0].runs[0].props.normalize_height = Some(false);
    disabled.text = Some(disabled_body);
    let mut absent = shape(&mut p, "Absent", 10.0, 190.0, 200.0, 80.0);
    absent.text = Some(TextBody::from_text("absent"));
    slide.shapes = vec![enabled, disabled, absent];
    p.slides.push(Arc::new(slide));

    let twice = round(&round(&p));
    let slide = twice.slides.last().expect("added slide");
    let run = &by_name(slide, "Enabled").text.as_ref().expect("text").paragraphs[0].runs[0];
    assert_eq!(run.props.normalize_height, Some(true));
    assert_eq!(run.props.size, Some(18.0));
    assert_eq!(run.props.caps, Some(Caps::Small));
    assert_eq!(run.props.spacing, Some(1.5));
    let run = &by_name(slide, "Disabled").text.as_ref().expect("text").paragraphs[0].runs[0];
    assert_eq!(run.props.normalize_height, Some(false));
    let run = &by_name(slide, "Absent").text.as_ref().expect("text").paragraphs[0].runs[0];
    assert_eq!(run.props.normalize_height, None);
}

#[test]
fn thai_distributed_alignment_survives_two_pptx_round_trips() {
    assert_eq!(Align::from_xml("thaiDist"), Some(Align::ThaiDistributed));
    assert_eq!(Align::ThaiDistributed.xml(), "thaiDist");
    assert_eq!(Align::from_xml("just"), Some(Align::Justify));

    let mut p = Presentation::default();
    let mut slide = new_slide(&mut p, "Thai distributed alignment");
    let mut text = shape(&mut p, "Text", 10.0, 10.0, 200.0, 80.0);
    let mut body = TextBody::from_text("one two three");
    body.paragraphs[0].props.align = Some(Align::ThaiDistributed);
    text.text = Some(body);
    slide.shapes.push(text);
    p.slides.push(Arc::new(slide));

    let twice = round(&round(&p));
    let slide = twice.slides.last().expect("added slide");
    let text = by_name(slide, "Text").text.as_ref().expect("text body");
    assert_eq!(text.paragraphs[0].props.align, Some(Align::ThaiDistributed));
}

#[test]
fn compatible_line_spacing_mode_survives_two_pptx_round_trips() {
    let mut p = Presentation::default();
    let mut slide = new_slide(&mut p, "Compatible line spacing");
    let mut enabled = shape(&mut p, "Enabled", 10.0, 10.0, 200.0, 80.0);
    let mut enabled_body = TextBody::from_text("enabled");
    enabled_body.body.compat_ln_spc = Some(true);
    enabled.text = Some(enabled_body);
    let mut disabled = shape(&mut p, "Disabled", 10.0, 100.0, 200.0, 80.0);
    let mut disabled_body = TextBody::from_text("disabled");
    disabled_body.body.compat_ln_spc = Some(false);
    disabled.text = Some(disabled_body);
    slide.shapes = vec![enabled, disabled];
    p.slides.push(Arc::new(slide));

    let twice = round(&round(&p));
    let slide = twice.slides.last().expect("added slide");
    assert_eq!(by_name(slide, "Enabled").text.as_ref().expect("text").body.compat_ln_spc, Some(true));
    assert_eq!(by_name(slide, "Disabled").text.as_ref().expect("text").body.compat_ln_spc, Some(false));
}

#[test]
fn text_overflow_policies_survive_two_pptx_round_trips() {
    let mut p = Presentation::default();
    let mut slide = new_slide(&mut p, "Text overflow");
    let mut text = shape(&mut p, "Text", 10.0, 10.0, 200.0, 80.0);
    let mut body = TextBody::from_text("policy");
    body.body.vert_overflow = Some(TextOverflow::Clip);
    body.body.horz_overflow = Some(TextOverflow::Overflow);
    text.text = Some(body);
    let mut table_shape = shape(&mut p, "Table", 10.0, 100.0, 200.0, 80.0);
    let mut table = Table::new(1, 1, 200.0, 80.0);
    table.rows[0].cells[0].horz_overflow = Some(TextOverflow::Clip);
    table_shape.kind = ShapeKind::Table(table);
    slide.shapes = vec![text, table_shape];
    p.slides.push(Arc::new(slide));

    let twice = round(&round(&p));
    let slide = twice.slides.last().expect("added slide");
    let text = by_name(slide, "Text").text.as_ref().expect("text body");
    assert_eq!(text.body.vert_overflow, Some(TextOverflow::Clip));
    assert_eq!(text.body.horz_overflow, Some(TextOverflow::Overflow));
    let ShapeKind::Table(table) = &by_name(slide, "Table").kind else { panic!("table") };
    assert_eq!(table.rows[0].cells[0].horz_overflow, Some(TextOverflow::Clip));
}

#[test]
fn horizontal_ellipsis_is_not_written_because_only_vertical_overflow_has_it() {
    let mut p = Presentation::default();
    let mut slide = new_slide(&mut p, "Horizontal ellipsis");
    let mut text = shape(&mut p, "Text", 10.0, 10.0, 200.0, 80.0);
    let mut body = TextBody::from_text("policy");
    body.body.vert_overflow = Some(TextOverflow::Ellipsis);
    body.body.horz_overflow = Some(TextOverflow::Ellipsis);
    text.text = Some(body);
    slide.shapes = vec![text];
    p.slides.push(Arc::new(slide));

    let once = round(&p);
    let slide = once.slides.last().expect("added slide");
    let text = by_name(slide, "Text").text.as_ref().expect("text body");
    assert_eq!(text.body.vert_overflow, Some(TextOverflow::Ellipsis));
    assert_eq!(text.body.horz_overflow, None);
}

#[test]
fn shapes_fills_lines_effects_round_trip() {
    let p = rich_deck();
    let q = round(&p);
    for name in ["Gradient Star", "Pattern Box", "Freeform", "Uses background"] {
        let (a, b) = (by_name(&p.slides[1], name), by_name(&q.slides[1], name));
        assert_eq!(a.geom, b.geom, "{name}");
        assert_eq!(a.fill, b.fill, "{name}");
        assert_eq!(a.line, b.line, "{name}");
        assert_eq!(a.effects, b.effects, "{name}");
        assert_eq!(a.xfrm, b.xfrm, "{name}");
    }
    let g = by_name(&q.slides[1], "Group");
    assert!(matches!(&g.kind, ShapeKind::Group { children, child } if children.len() == 2 && child.w == 110.0));
    let c = by_name(&q.slides[1], "Connector");
    let (c1, c2) = (by_name(&q.slides[1], "Child 1").id, by_name(&q.slides[1], "Child 2").id);
    assert!(matches!(c.kind, ShapeKind::Connector { start: Some((s, 2)), end: Some((e, 0)) } if s == c1 && e == c2), "{:?}", c.kind);
    assert_eq!(c.line, by_name(&p.slides[1], "Connector").line);
    // Ids are kept.
    assert_eq!(by_name(&q.slides[1], "Group").id, by_name(&p.slides[1], "Group").id);
}

#[test]
fn bullet_size_in_points_survives_two_pptx_round_trips() {
    let mut p = Presentation::default();
    let mut textbox = shape(&mut p, "Point-sized bullet", 20.0, 20.0, 400.0, 100.0);
    textbox.text = Some(TextBody {
        paragraphs: [12.0, 500.0, 4000.0]
            .into_iter()
            .map(|size| Paragraph {
                props: deckcraft_model::text::ParaProps {
                    bullet: Some(Bullet::Char { char: "•".into() }),
                    bullet_size_points: Some(size),
                    ..Default::default()
                },
                runs: vec![Run::with("Bullet text", RunProps::default())],
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    });
    Arc::make_mut(&mut p.slides[0]).shapes.push(textbox);

    let q = round(&round(&p));
    let props = &by_name(&q.slides[0], "Point-sized bullet").text.as_ref().expect("text").paragraphs;
    assert_eq!(props.iter().map(|p| p.props.bullet_size_points).collect::<Vec<_>>(), vec![Some(12.0), Some(500.0), Some(4000.0)]);
    assert!(props.iter().all(|p| p.props.bullet_size.is_none()));
}

#[test]
fn line_miter_limit_survives_two_pptx_round_trips() {
    let mut p = Presentation::default();
    let mut limited = shape(&mut p, "Limited miter", 20.0, 20.0, 100.0, 100.0);
    limited.line = Some(Line { join: Some(deckcraft_model::style::LineJoin::Miter), miter_limit: Some(200_000), ..Default::default() });
    let mut defaulted = shape(&mut p, "Default miter", 140.0, 20.0, 100.0, 100.0);
    defaulted.line = Some(Line { join: Some(deckcraft_model::style::LineJoin::Miter), ..Default::default() });
    let mut large = shape(&mut p, "Large miter", 260.0, 20.0, 100.0, 100.0);
    large.line = Some(Line { join: Some(deckcraft_model::style::LineJoin::Miter), miter_limit: Some(1_000_001), ..Default::default() });
    Arc::make_mut(&mut p.slides[0]).shapes.extend([limited, defaulted, large]);

    let q = round(&round(&p));
    assert_eq!(by_name(&q.slides[0], "Limited miter").line.as_ref().and_then(|line| line.miter_limit), Some(200_000));
    assert_eq!(by_name(&q.slides[0], "Default miter").line.as_ref().and_then(|line| line.miter_limit), None);
    assert_eq!(by_name(&q.slides[0], "Large miter").line.as_ref().and_then(|line| line.miter_limit), Some(1_000_001));
}

#[test]

fn bullet_follow_text_resets_survive_two_pptx_round_trips() {
    let mut p = Presentation::default();
    let mut textbox = shape(&mut p, "Follow text bullets", 20.0, 20.0, 400.0, 100.0);
    textbox.text = Some(TextBody {
        paragraphs: vec![Paragraph {
            props: deckcraft_model::text::ParaProps {
                bullet: Some(Bullet::Char { char: "•".into() }),
                bullet_color_follow_text: Some(true),
                bullet_size_follow_text: Some(true),
                bullet_font_follow_text: Some(true),
                ..Default::default()
            },
            runs: vec![Run::with("Bullet text", RunProps::default())],
            ..Default::default()
        }],
        ..Default::default()
    });
    Arc::make_mut(&mut p.slides[0]).shapes.push(textbox);
    let mut conflicting = shape(&mut p, "Explicit bullet values", 20.0, 140.0, 400.0, 100.0);
    conflicting.text = Some(TextBody {
        paragraphs: vec![Paragraph {
            props: deckcraft_model::text::ParaProps {
                bullet: Some(Bullet::Char { char: "•".into() }),
                bullet_color: Some(deckcraft_model::style::ColorRef::rgb(deckcraft_color::Rgba::rgb(255, 0, 0))),
                bullet_color_follow_text: Some(true),
                bullet_size: Some(1.5),
                bullet_size_follow_text: Some(true),
                bullet_font: Some("Inter".into()),
                bullet_font_follow_text: Some(true),
                ..Default::default()
            },
            runs: vec![Run::with("Explicit bullet text", RunProps::default())],
            ..Default::default()
        }],
        ..Default::default()
    });
    Arc::make_mut(&mut p.slides[0]).shapes.push(conflicting);

    let q = round(&round(&p));
    let props = &by_name(&q.slides[0], "Follow text bullets").text.as_ref().expect("text").paragraphs[0].props;
    assert_eq!(props.bullet_color_follow_text, Some(true));
    assert_eq!(props.bullet_size_follow_text, Some(true));
    assert_eq!(props.bullet_font_follow_text, Some(true));
    let explicit = &by_name(&q.slides[0], "Explicit bullet values").text.as_ref().expect("text").paragraphs[0].props;
    assert_eq!(explicit.bullet_color_follow_text, None);
    assert_eq!(explicit.bullet_size_follow_text, None);
    assert_eq!(explicit.bullet_font_follow_text, None);
    assert_eq!(explicit.bullet_size, Some(1.5));
    assert_eq!(explicit.bullet_font.as_deref(), Some("Inter"));
    assert_eq!(explicit.bullet_color, Some(deckcraft_model::style::ColorRef::rgb(deckcraft_color::Rgba::rgb(255, 0, 0))));
}

#[test]

fn line_alignment_survives_two_pptx_round_trips() {
    let mut p = Presentation::default();
    let mut inside = shape(&mut p, "Inset line", 20.0, 20.0, 100.0, 100.0);
    inside.line = Some(Line { align: Some(deckcraft_model::style::LineAlign::Inside), ..Default::default() });
    let mut centered = shape(&mut p, "Centered line", 140.0, 20.0, 100.0, 100.0);
    centered.line = Some(Line { align: Some(deckcraft_model::style::LineAlign::Center), ..Default::default() });
    Arc::make_mut(&mut p.slides[0]).shapes.extend([inside, centered]);

    let q = round(&round(&p));
    assert_eq!(by_name(&q.slides[0], "Inset line").line.as_ref().and_then(|line| line.align), Some(deckcraft_model::style::LineAlign::Inside));
    assert_eq!(by_name(&q.slides[0], "Centered line").line.as_ref().and_then(|line| line.align), Some(deckcraft_model::style::LineAlign::Center));
    assert_eq!(deckcraft_model::style::LineAlign::from_xml("out"), None);
}

#[test]

fn gradient_flip_mode_survives_two_pptx_round_trips() {
    let p = rich_deck();
    let q = round(&round(&p));
    let gradient = by_name(&q.slides[1], "Gradient Star").fill.as_ref();
    assert!(matches!(gradient, Some(Fill::Gradient(g)) if g.flip == Some(GradientFlip::Xy)));
}

#[test]
fn pictures_media_and_ink_round_trip() {
    let p = rich_deck();
    let q = round(&p);
    let (a, b) = (by_name(&p.slides[2], "Picture"), by_name(&q.slides[2], "Picture"));
    let (ShapeKind::Picture { fill: fa }, ShapeKind::Picture { fill: fb }) = (&a.kind, &b.kind) else { panic!("not pictures") };
    assert_eq!(fa.crop, fb.crop);
    assert_eq!(fa.alpha, fb.alpha);
    assert_eq!(fa.adjust, fb.adjust);
    assert_eq!(p.media(fa.media).map(|m| m.data.clone()), q.media(fb.media).map(|m| m.data.clone()));
    let t = by_name(&q.slides[2], "Tiled");
    assert!(matches!(&t.fill, Some(Fill::Picture(pf)) if matches!(pf.mode, PictureMode::Tile { sx, .. } if sx == 0.5)));
    let v = by_name(&q.slides[2], "Video");
    let ShapeKind::Media(m) = &v.kind else { panic!("not media: {:?}", v.kind) };
    assert!(m.video);
    assert_eq!((m.trim_start_ms, m.fade_in_ms), (500, 250));
    assert!(q.media(m.media).is_some_and(|x| x.data.starts_with(b"\x00\x00\x00\x18ftyp")));
    assert!(m.poster.is_some());
    // The crop of the video survives the round trip (#972).
    for (got, want) in m.crop.iter().zip([0.1, 0.0, 0.2, 0.25]) {
        assert!((got - want).abs() < 1e-4, "video crop {:?}", m.crop);
    }
    // Ink comes back as freeform lines in a group. A flat 0.5 stroke keeps the pen width.
    let ink = by_name(&q.slides[2], "Ink");
    let ShapeKind::Group { children, .. } = &ink.kind else { panic!("ink is not a group: {:?}", ink.kind) };
    assert_eq!(children.len(), 1);
    assert!(matches!(children[0].geom, Geom::Custom { .. }));
    let w = children[0].line.as_ref().and_then(|l| l.width).unwrap_or(0.0);
    assert!((w - 3.0).abs() < 0.05, "a flat 0.5 stroke keeps the pen width, got {w}");
}

#[test]
fn pressured_ink_exports_at_its_mean_width() {
    let mut p = Presentation::default();
    let id = ShapeId(p.alloc_id().expect("id"));
    let slide = std::sync::Arc::make_mut(&mut p.slides[0]);
    slide.shapes.push(Shape {
        id,
        name: "Ink".into(),
        xfrm: Some(Xfrm::new(0.0, 0.0, 960.0, 540.0)),
        kind: ShapeKind::Ink {
            strokes: vec![InkStroke {
                points: vec![(10.0, 10.0, 1.0), (40.0, 10.0, 0.5), (80.0, 10.0, 0.0)],
                color: Rgba::rgb(0, 0, 0),
                width: 10.0,
                highlighter: false,
            }],
        },
        ..Default::default()
    });
    let q = round(&p);
    let ink = by_name(&q.slides[0], "Ink");
    let ShapeKind::Group { children, .. } = &ink.kind else { panic!("{:?}", ink.kind) };
    let w = children[0].line.as_ref().and_then(|l| l.width).unwrap_or(0.0);
    // Full, half, and the 8% floor: (10 + 5 + 0.8) / 3.
    assert!((w - (10.0 + 5.0 + 0.8) / 3.0).abs() < 0.05, "{w}");
}

#[test]
fn tables_round_trip() {
    let p = rich_deck();
    let q = round(&p);
    let (ShapeKind::Table(a), ShapeKind::Table(b)) = (&by_name(&p.slides[3], "Table 1").kind, &by_name(&q.slides[3], "Table 1").kind) else {
        panic!()
    };
    // A cell always has at least one paragraph in the file; compare without empty ones.
    let norm = |t: &Table| {
        let mut t = t.clone();
        for r in &mut t.rows {
            for c in &mut r.cells {
                c.text.paragraphs.retain(|p| !p.is_empty());
            }
        }
        t
    };
    assert_eq!(norm(a), norm(b));
}

#[test]
fn charts_round_trip() {
    let p = rich_deck();
    let q = round(&p);
    for (sa, sb) in p.slides[4].shapes.iter().zip(q.slides[4].shapes.iter()) {
        let (ShapeKind::Chart(a), ShapeKind::Chart(b)) = (&sa.kind, &sb.kind) else { continue };
        assert_eq!(a.kind, b.kind, "{}", sa.name);
        assert_eq!(a.title, b.title);
        assert_eq!(a.series.len(), b.series.len());
        for (x, y) in a.series.iter().zip(b.series.iter()) {
            assert_eq!(x.name, y.name);
            assert_eq!(x.values, y.values, "{}", sa.name);
        }
        if !matches!(a.kind, ChartType::Scatter | ChartType::Bubble) {
            assert_eq!(a.categories, b.categories);
        }
        assert!(b.raw.is_some());
    }
    // Second export reuses the kept chart XML unchanged.
    let r = round(&q);
    for (sa, sb) in q.slides[4].shapes.iter().zip(r.slides[4].shapes.iter()) {
        if let (ShapeKind::Chart(a), ShapeKind::Chart(b)) = (&sa.kind, &sb.kind) {
            let (mut a, mut b) = (a.clone(), b.clone());
            a.raw = None;
            b.raw = None;
            assert_eq!(a, b);
        }
    }
}

#[test]
fn transitions_and_animations_round_trip() {
    let p = rich_deck();
    let q = round(&p);
    for (a, b) in p.slides.iter().zip(q.slides.iter()) {
        match (&a.transition, &b.transition) {
            (Some(x), Some(y)) => {
                assert_eq!(
                    (&x.kind, &x.option, x.duration_ms, x.advance_on_click, x.advance_after_ms),
                    (&y.kind, &y.option, y.duration_ms, y.advance_on_click, y.advance_after_ms)
                );
            }
            (None, None) => {}
            other => panic!("{other:?}"),
        }
        assert_eq!(a.animations.len(), b.animations.len());
        for (x, y) in a.animations.iter().zip(b.animations.iter()) {
            assert_eq!(
                (&x.effect, &x.option, x.class, x.start, x.duration_ms, x.delay_ms, x.text_build),
                (&y.effect, &y.option, y.class, y.start, y.duration_ms, y.delay_ms, y.text_build)
            );
            assert_eq!(x.shape, y.shape);
            assert_eq!(x.trigger, y.trigger);
            if x.class == AnimClass::Path {
                assert_eq!(x.path, y.path);
            }
        }
    }
}

#[test]
fn comments_round_trip() {
    let p = rich_deck();
    let q = round(&p);
    let c = &q.slides[1].comments;
    assert_eq!(c.len(), 1);
    assert_eq!((c[0].author.as_str(), c[0].initials.as_str(), c[0].text.as_str()), ("Ada", "AL", "Nice star"));
    assert_eq!((c[0].x, c[0].y), (120.0, 64.0));
}

#[test]
fn every_transition_kind_survives() {
    let mut p = defaults::new_presentation(None);
    let base = p.slides[0].as_ref().clone();
    p.slides.clear();
    for (kind, ..) in deckcraft_model::anim::TRANSITIONS {
        let mut s = base.clone();
        s.id = SlideId(p.alloc_id().expect("ids"));
        s.transition = Some(Transition { kind: kind.to_string(), duration_ms: 1200, ..Default::default() });
        p.slides.push(Arc::new(s));
    }
    let q = round(&p);
    for (a, b) in p.slides.iter().zip(q.slides.iter()) {
        let k = a.transition.as_ref().map(|t| t.kind.clone()).unwrap_or_default();
        if k == "none" {
            continue;
        }
        assert_eq!(b.transition.as_ref().map(|t| t.kind.clone()), Some(k.clone()));
        assert_eq!(b.transition.as_ref().map(|t| t.duration_ms), Some(1200), "{k}");
    }
}

#[test]
fn every_animation_effect_survives() {
    let mut p = defaults::new_presentation(None);
    let mut s = p.slides[0].as_ref().clone();
    let mut sh = shape(&mut p, "Target", 100.0, 100.0, 100.0, 100.0);
    sh.text = Some(TextBody::from_text("hi"));
    let id = sh.id;
    s.shapes.push(sh);
    for (effect, _, class, _, dur, opts) in deckcraft_model::anim::ANIMATIONS {
        s.animations.push(Animation {
            shape: id,
            class: *class,
            effect: effect.to_string(),
            option: opts.first().map(|o| o.to_string()).unwrap_or_default(),
            duration_ms: (*dur).max(1),
            ..Default::default()
        });
    }
    p.slides[0] = Arc::new(s);
    let q = round(&p);
    let (a, b) = (&p.slides[0].animations, &q.slides[0].animations);
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(b.iter()) {
        // Effects sharing a preset number come back as the first effect with that number.
        let same_preset = deckcraft_model::anim::animation_info(&x.effect, x.class).map(|i| i.3)
            == deckcraft_model::anim::animation_info(&y.effect, y.class).map(|i| i.3);
        assert!(x.effect == y.effect || same_preset, "{} vs {}", x.effect, y.effect);
        assert_eq!(x.class, y.class);
    }
}

#[test]
fn animation_rewind_survives() {
    let mut p = defaults::new_presentation(None);
    let mut s = p.slides[0].as_ref().clone();
    let sh = shape(&mut p, "Target", 100.0, 100.0, 100.0, 100.0);
    let id = sh.id;
    s.shapes.push(sh);
    s.animations.push(Animation { shape: id, rewind: true, ..Default::default() });
    s.animations.push(Animation { shape: id, effect: "fly".into(), option: "l".into(), start: AnimStart::WithPrevious, ..Default::default() });
    p.slides[0] = Arc::new(s);
    let q = round(&p);
    let b = &q.slides[0].animations;
    assert_eq!(b.len(), 2);
    assert!(b[0].rewind);
    assert!(!b[1].rewind);
}

/// Writes bisection variants of the rich deck to `DECKCRAFT_PPTX_BISECT` (manual checks).
#[test]
fn bisect_variants() {
    let Ok(dir) = std::env::var("DECKCRAFT_PPTX_BISECT") else { return };
    let p = rich_deck();
    let n = p.slides.len();
    for i in 0..n {
        let mut q = p.clone();
        q.slides = vec![p.slides[i].clone()];
        q.sections.clear();
        q.custom_shows.clear();
        std::fs::write(format!("{dir}/bis-slide{i}.pptx"), deckcraft_pptx::export(&q).expect("export")).expect("write");
    }
    let mut q = p.clone();
    q.sections.clear();
    q.custom_shows.clear();
    q.first_slide_number = 1;
    for s in q.slides.iter_mut() {
        Arc::make_mut(s).comments.clear();
    }
    std::fs::write(format!("{dir}/bis-nodeck.pptx"), deckcraft_pptx::export(&q).expect("export")).expect("write");
}

/// Finer bisection of one slide (`DECKCRAFT_PPTX_BISECT_SLIDE=<index>`): one variant per removed shape,
/// plus no animations / no transition.
#[test]
fn bisect_slide_variants() {
    let (Ok(dir), Ok(idx)) = (std::env::var("DECKCRAFT_PPTX_BISECT"), std::env::var("DECKCRAFT_PPTX_BISECT_SLIDE")) else { return };
    let p = rich_deck();
    let i: usize = idx.parse().expect("index");
    let base = p.slides[i].as_ref().clone();
    let one = |s: Slide| {
        let mut q = p.clone();
        q.slides = vec![Arc::new(s)];
        q.sections.clear();
        q.custom_shows.clear();
        deckcraft_pptx::export(&q).expect("export")
    };
    let mut s = base.clone();
    s.animations.clear();
    std::fs::write(format!("{dir}/v-noanim.pptx"), one(s)).expect("write");
    let mut s = base.clone();
    s.transition = None;
    std::fs::write(format!("{dir}/v-notrans.pptx"), one(s)).expect("write");
    for k in 0..base.shapes.len() {
        let mut s = base.clone();
        s.animations.clear();
        s.transition = None;
        let keep = s.shapes[k].clone();
        s.shapes.retain(|x| x.id == keep.id);
        std::fs::write(format!("{dir}/v-only{k}.pptx"), one(s)).expect("write");
    }
}

#[test]
fn rtl_paragraphs_script_fonts_and_just_low_round_trip() {
    let mut p = rich_deck();
    let fonts = &mut Arc::make_mut(&mut p.masters[0]).theme.fonts;
    fonts.minor.cs = "Noto Naskh Arabic".into();
    fonts.minor.scripts = vec![
        deckcraft_model::theme::ScriptFont { script: "Arab".into(), typeface: "Noto Naskh Arabic".into() },
        deckcraft_model::theme::ScriptFont { script: "Hebr".into(), typeface: "Noto Sans Hebrew".into() },
    ];
    let slide = Arc::make_mut(&mut p.slides[0]);
    let sh = slide.shapes.iter_mut().find(|s| s.text.is_some()).expect("a text shape");
    let body = sh.text.as_mut().expect("text");
    body.paragraphs = vec![Paragraph { runs: vec![Run::new("سنة 2024 (Retrait) فقط")], ..Default::default() }];
    body.paragraphs[0].props.rtl = Some(true);
    body.paragraphs[0].props.align = Some(Align::JustLow);
    let name = sh.name.clone();
    let q = round(&p);
    assert_eq!(q.masters[0].theme.fonts.minor.scripts, p.masters[0].theme.fonts.minor.scripts);
    assert_eq!(q.masters[0].theme.fonts.minor.script("arab"), Some("Noto Naskh Arabic"));
    let para = &by_name(&q.slides[0], &name).text.as_ref().expect("text").paragraphs[0];
    assert_eq!(para.props.rtl, Some(true));
    assert_eq!(para.props.align, Some(Align::JustLow));
    assert_eq!(para.runs[0].text, "سنة 2024 (Retrait) فقط");
}

#[test]
fn date_fields_keep_their_type_and_saved_text() {
    // An automatic date is a field (its saved text is what readers without a clock show); a fixed
    // date is plain text.
    let mut p = Presentation::default();
    let mut auto = shape(&mut p, "Auto date", 10.0, 10.0, 200.0, 40.0);
    let mut body = TextBody::from_text("");
    body.paragraphs[0].runs =
        vec![Run { text: "March 3, 2025".into(), props: RunProps::default(), kind: RunKind::Field { field: "datetime4".into() } }];
    auto.text = Some(body);
    let mut fixed = shape(&mut p, "Fixed date", 10.0, 60.0, 200.0, 40.0);
    fixed.text = Some(TextBody::from_text("Launch day"));
    Arc::make_mut(&mut p.slides[0]).shapes = vec![auto, fixed];
    let q = round(&p);
    for name in ["Auto date", "Fixed date"] {
        assert_eq!(by_name(&q.slides[0], name).text, by_name(&p.slides[0], name).text, "{name}");
    }
}

#[test]
fn colour_map_overrides_round_trip() {
    let mut p = defaults::new_presentation(None);
    let dark: Vec<(String, String)> =
        [("bg1", "dk1"), ("tx1", "lt1"), ("bg2", "dk2"), ("tx2", "lt2")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
    let layout = p.slides[0].layout;
    std::sync::Arc::make_mut(&mut p.masters[0]).layouts.iter_mut().find(|l| l.id == layout).expect("layout").color_map = Some(dark.clone());
    let first = p.slides[0].id;
    p.slide_mut(first).expect("slide").color_map = Some(dark.clone());
    let q = round(&p);
    let get = |m: &Option<Vec<(String, String)>>, k: &str| m.as_ref().and_then(|m| m.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone()));
    let ql = q.masters[0].layouts.iter().find(|l| l.color_map.is_some()).expect("the layout keeps its override");
    assert_eq!(get(&ql.color_map, "bg1").as_deref(), Some("dk1"));
    assert_eq!(get(&q.slides[0].color_map, "tx1").as_deref(), Some("lt1"));
    // Layouts and slides without one keep following the master.
    assert!(q.masters[0].layouts.iter().filter(|l| l.color_map.is_none()).count() >= 1);
}

#[test]
fn a_picture_duotone_round_trips() {
    let mut p = defaults::new_presentation(None);
    let img = p.add_media("image1.png", "image/png", png(4, 4, [128, 128, 128])).expect("media");
    let mut pic = shape(&mut p, "Duotone", 10.0, 10.0, 100.0, 100.0);
    let mut pf = PictureFill { media: img, ..Default::default() };
    pf.adjust.duotone = Some((ColorRef::scheme(SchemeSlot::Accent1).with(ColorTransform::Shade(30000)), ColorRef::rgb(Rgba::rgb(250, 240, 200))));
    pic.kind = deckcraft_model::ShapeKind::Picture { fill: pf };
    let first = p.slides[0].id;
    p.slide_mut(first).expect("slide").shapes.push(pic);
    let q = round(&p);
    let sh = by_name(&q.slides[0], "Duotone");
    let deckcraft_model::ShapeKind::Picture { fill } = &sh.kind else { panic!("picture") };
    let (d, l) = fill.adjust.duotone.as_ref().expect("duotone kept");
    assert!(matches!(d.base, deckcraft_model::ColorBase::Scheme { slot: SchemeSlot::Accent1 }));
    assert!(matches!(l.base, deckcraft_model::ColorBase::Rgb { rgb } if rgb == Rgba::rgb(250, 240, 200)));
}

#[test]
fn line_break_options_round_trip() {
    let mut p = defaults::new_presentation(None);
    let first = p.slides[0].id;
    let s = p.slide_mut(first).expect("slide");
    let t = s.shapes[0].text.insert(TextBody::from_text("x"));
    t.paragraphs[0].props.latin_line_break = Some(true);
    t.paragraphs[0].props.hanging_punct = Some(false);
    let q = round(&p);
    let props = &q.slides[0].shapes[0].text.as_ref().expect("text").paragraphs[0].props;
    assert_eq!(props.latin_line_break, Some(true));
    assert_eq!(props.hanging_punct, Some(false));
}

#[test]
fn a_table_fill_round_trips() {
    let mut p = defaults::new_presentation(None);
    let mut t = Table::new(2, 2, 200.0, 40.0);
    t.background = Some(Fill::solid(ColorRef::rgb(Rgba::rgb(0, 112, 192))));
    let mut sh = shape(&mut p, "Filled table", 10.0, 10.0, 200.0, 80.0);
    sh.kind = deckcraft_model::ShapeKind::Table(t);
    let first = p.slides[0].id;
    p.slide_mut(first).expect("slide").shapes.push(sh);
    let q = round(&p);
    let deckcraft_model::ShapeKind::Table(t) = &by_name(&q.slides[0], "Filled table").kind else { panic!("table") };
    assert!(
        matches!(&t.background, Some(Fill::Solid { color }) if matches!(color.base, deckcraft_model::ColorBase::Rgb { rgb } if rgb == Rgba::rgb(0, 112, 192)))
    );
}

#[test]
fn wordart_warps_and_their_adjusts_round_trip() {
    let mut p = Presentation::default();
    let cases: [(&str, &str, Vec<f64>); 5] = [
        ("Arch pour", "textArchUpPour", vec![13_500_000.0, 30_000.0]),
        ("Wave", "textWave1", vec![18_000.0, -5_000.0]),
        ("Inflate", "textInflate", vec![30_000.0]),
        ("Arch", "textArchUp", vec![]),
        ("Ring", "textRingInside", vec![75_000.0]),
    ];
    let mut shapes = vec![];
    for (i, (name, warp, adj)) in cases.iter().enumerate() {
        let mut s = shape(&mut p, name, 10.0, 10.0 + 50.0 * i as f64, 300.0, 40.0);
        let mut body = TextBody::from_text("WordArt");
        body.body.warp = Some((*warp).into());
        body.body.warp_adj = adj.clone();
        s.text = Some(body);
        shapes.push(s);
    }
    Arc::make_mut(&mut p.slides[0]).shapes = shapes;
    let q = round(&p);
    for (name, warp, adj) in &cases {
        let b = &by_name(&q.slides[0], name).text.as_ref().expect("text").body;
        assert_eq!(b.warp.as_deref(), Some(*warp), "{name}");
        assert_eq!(&b.warp_adj, adj, "{name}");
    }
    // The single-adjust preset is written with the guide name `adj`, two-adjust ones with
    // `adj1`/`adj2`, and a preset with no adjusts given gets an empty list.
    let bytes = deckcraft_pptx::export(&p).expect("export");
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("zip");
    let mut xml = String::new();
    std::io::Read::read_to_string(&mut zip.by_name("ppt/slides/slide1.xml").expect("slide"), &mut xml).expect("read");
    assert!(xml.contains(r#"<a:prstTxWarp prst="textInflate"><a:avLst><a:gd name="adj" fmla="val 30000"/></a:avLst></a:prstTxWarp>"#), "{xml}");
    assert!(xml.contains(r#"<a:gd name="adj1" fmla="val 13500000"/><a:gd name="adj2" fmla="val 30000"/>"#));
    assert!(xml.contains(r#"<a:prstTxWarp prst="textArchUp"><a:avLst/></a:prstTxWarp>"#));
    // A file that gives only `adj2` keeps the preset's default for `adj1`; hostile values are
    // read as numbers or dropped.
    let edited = xml
        .replace(r#"<a:gd name="adj1" fmla="val 13500000"/>"#, "")
        .replace(r#"<a:gd name="adj" fmla="val 30000"/>"#, r#"<a:gd name="adj" fmla="val NaN"/>"#);
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).expect("entry");
        let name = f.name().to_string();
        let mut data = vec![];
        std::io::Read::read_to_end(&mut f, &mut data).expect("read");
        if name == "ppt/slides/slide1.xml" {
            data = edited.clone().into_bytes();
        }
        out.start_file(name, zip::write::SimpleFileOptions::default()).expect("start");
        std::io::Write::write_all(&mut out, &data).expect("write");
    }
    let bytes = out.finish().expect("finish").into_inner();
    let q = deckcraft_pptx::import(&bytes).expect("import");
    let pour = &by_name(&q.slides[0], "Arch pour").text.as_ref().expect("text").body;
    assert_eq!(pour.warp_adj, vec![10_800_000.0, 30_000.0]);
    let inflate = &by_name(&q.slides[0], "Inflate").text.as_ref().expect("text").body;
    assert_eq!(inflate.warp.as_deref(), Some("textInflate"));
    assert!(inflate.warp_adj.is_empty());
}

/// Slide XML of a deck exported with one slide per transition (kind, option, ms, advance after).
fn transition_deck(list: &[(&str, &str, u32, Option<u32>)]) -> (Presentation, Vec<String>) {
    let mut p = defaults::new_presentation(None);
    let base = p.slides[0].as_ref().clone();
    p.slides.clear();
    for (kind, option, ms, adv) in list {
        let mut s = base.clone();
        s.id = SlideId(p.alloc_id().expect("ids"));
        s.transition = Some(Transition {
            kind: kind.to_string(),
            option: option.to_string(),
            duration_ms: *ms,
            advance_on_click: adv.is_none(),
            advance_after_ms: *adv,
            ..Default::default()
        });
        p.slides.push(Arc::new(s));
    }
    let bytes = deckcraft_pptx::export(&p).expect("export");
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("zip");
    let xml = (1..=list.len())
        .map(|i| {
            let mut x = String::new();
            std::io::Read::read_to_string(&mut zip.by_name(&format!("ppt/slides/slide{i}.xml")).expect("slide"), &mut x).expect("read");
            x
        })
        .collect();
    (p, xml)
}

#[test]
fn transition_xml_matches_powerpoints_elements() {
    // The elements, attributes and namespaces PowerPoint itself writes for these effect options
    // (observed on files it saved), inside `mc:AlternateContent` with a `p:fade` fallback for
    // the 2010+ effects.
    let cases: &[(&str, &str, &[&str], &str)] = &[
        ("wipe", "ru", &[r#"<p:strips dir="ru"/>"#], "p14"),
        ("wipe", "u", &[r#"<p:wipe dir="u"/>"#], "p14"),
        ("zoom", "in", &[r#"<p14:warp dir="in"/>"#], "p14"),
        ("zoom", "rotate", &["<p:newsflash/>"], "p14"),
        ("shape", "out", &[r#"<p:zoom dir="out"/>"#], "p14"),
        ("glitter", "hexD", &[r#"<p14:glitter dir="d" pattern="hexagon"/>"#], "p14"),
        ("shred", "particlesOut", &[r#"<p14:shred pattern="rectangle" dir="out"/>"#], "p14"),
        ("wind", "l", &[r#"<p15:prstTrans prst="wind" invX="1"/>"#], "p15"),
        ("wind", "r", &[r#"<p15:prstTrans prst="wind"/>"#], "p15"),
        ("fallOver", "r", &[r#"<p15:prstTrans prst="fallOver" invX="1"/>"#], "p15"),
        ("pageCurlDouble", "singleR", &[r#"<p15:prstTrans prst="pageCurlSingle" invX="1"/>"#], "p15"),
        ("orbit", "u", &[r#"<p14:prism dir="u" isContent="1" isInverted="1"/>"#], "p14"),
        ("vortex", "r", &[r#"<p14:vortex dir="r"/>"#], "p14"),
        ("morph", "words", &[r#"<p159:morph option="byWord"/>"#], "p159"),
    ];
    let list: Vec<(&str, &str, u32, Option<u32>)> = cases.iter().map(|c| (c.0, c.1, 2000, Some(1000))).collect();
    let (p, xml) = transition_deck(&list);
    for ((kind, option, wants, req), x) in cases.iter().zip(&xml) {
        for w in *wants {
            assert!(x.contains(w), "{kind}/{option}: {w} in {x}");
        }
        assert!(x.contains(&format!(r#"Requires="{req}""#)), "{kind}/{option}");
        assert!(x.contains(r#"p14:dur="2000""#) && x.contains(r#"advTm="1000""#) && x.contains(r#"advClick="0""#), "{kind}/{option}");
        assert!(x.contains("<mc:Fallback>"), "{kind}/{option}");
    }
    // Back in, everything is as it was.
    let bytes = deckcraft_pptx::export(&p).expect("export");
    let q = deckcraft_pptx::import(&bytes).expect("import");
    for ((kind, option, ..), s) in cases.iter().zip(&q.slides) {
        let t = s.transition.as_ref().expect("transition");
        assert_eq!(
            (t.kind.as_str(), t.option.as_str(), t.duration_ms, t.advance_after_ms, t.advance_on_click),
            (*kind, *option, 2000, Some(1000), false)
        );
    }
}

#[test]
fn transition_defaults_read_like_powerpoint() {
    // PowerPoint leaves out an option when it is the element's default; these are its defaults.
    let cases: &[(&str, &str, &str)] = &[
        ("<p:push/>", "push", "l"),
        ("<p:wipe/>", "wipe", "l"),
        ("<p:strips/>", "wipe", "lu"),
        ("<p:randomBar/>", "randomBar", "horz"),
        ("<p:checker/>", "checker", "horz"),
        ("<p:blinds/>", "blinds", "horz"),
        ("<p:comb/>", "comb", "horz"),
        ("<p:zoom/>", "shape", "out"),
        (r#"<p:zoom dir="in"/>"#, "shape", "in"),
        ("<p:split/>", "split", "horzOut"),
        ("<p14:doors/>", "doors", "horz"),
        ("<p14:window/>", "window", "horz"),
        ("<p14:pan/>", "pan", "l"),
        ("<p14:warp/>", "zoom", "out"),
        ("<p14:glitter/>", "glitter", "l"),
        ("<p14:shred/>", "shred", "stripsIn"),
        ("<p14:reveal/>", "reveal", "smoothlyLeft"),
        ("<p14:prism/>", "cube", "l"),
        (r#"<p15:prstTrans prst="airplane"/>"#, "airplane", "r"),
        (r#"<p15:prstTrans prst="peelOff"/>"#, "peelOff", "l"),
        (r#"<p15:prstTrans prst="pageCurlSingle"/>"#, "pageCurlDouble", "singleL"),
    ];
    let (_, xml) = transition_deck(&[("fade", "smoothly", 2000, None)]);
    let Some(x) = xml.first() else { panic!("no slide") };
    let start = x.find("<mc:AlternateContent").expect("transition");
    let end = x.find("</mc:AlternateContent>").expect("transition end") + "</mc:AlternateContent>".len();
    let p14 = r#"xmlns:p14="http://schemas.microsoft.com/office/powerpoint/2010/main""#;
    let p15 = r#"xmlns:p15="http://schemas.microsoft.com/office/powerpoint/2012/main""#;
    for (el, kind, option) in cases {
        let tr = format!(r#"<p:transition {p14} {p15} spd="slow" p14:dur="2000">{el}</p:transition>"#);
        let edited = format!("{}{}{}", &x[..start], tr, &x[end..]);
        let mut p = defaults::new_presentation(None);
        let base = p.slides[0].as_ref().clone();
        p.slides = vec![Arc::new(base)];
        let bytes = deckcraft_pptx::export(&p).expect("export");
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("zip");
        let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for i in 0..zip.len() {
            let mut f = zip.by_index(i).expect("entry");
            let name = f.name().to_string();
            let mut data = vec![];
            std::io::Read::read_to_end(&mut f, &mut data).expect("read");
            if name == "ppt/slides/slide1.xml" {
                data = edited.clone().into_bytes();
            }
            out.start_file(name, zip::write::SimpleFileOptions::default()).expect("start");
            std::io::Write::write_all(&mut out, &data).expect("write");
        }
        let bytes = out.finish().expect("finish").into_inner();
        let q = deckcraft_pptx::import(&bytes).expect("import");
        let t = q.slides[0].transition.as_ref().expect("transition");
        assert_eq!((t.kind.as_str(), t.option.as_str(), t.duration_ms), (*kind, *option, 2000), "{el}");
    }
}

#[test]
fn linked_pictures_are_written_with_r_link() {
    // A picture whose bytes live outside the package must be referenced with `r:link`; `r:embed`
    // may only name a part inside the package (PowerPoint won't load the linked image otherwise).
    let mut p = defaults::new_presentation(None);
    let media = deckcraft_model::MediaId(5000);
    p.media.push(deckcraft_model::MediaItem {
        id: media,
        name: "linked.png".into(),
        content_type: "image/png".into(),
        data: Arc::new(Vec::new()),
        link: Some("file:///tmp/linked.png".into()),
    });
    let mut pic = shape(&mut p, "Linked", 100.0, 100.0, 200.0, 150.0);
    pic.kind = ShapeKind::Picture { fill: PictureFill { media, ..Default::default() } };
    Arc::make_mut(&mut p.slides[0]).shapes.push(pic);

    let bytes = deckcraft_pptx::export(&p).expect("export");
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("zip");
    let mut xml = String::new();
    std::io::Read::read_to_string(&mut zip.by_name("ppt/slides/slide1.xml").expect("slide"), &mut xml).expect("read");
    assert!(xml.contains(r#"r:link="rId"#), "{xml}");
    assert!(!xml.contains(r#"r:embed="rId"#), "{xml}");
    let mut rels = String::new();
    std::io::Read::read_to_string(&mut zip.by_name("ppt/slides/_rels/slide1.xml.rels").expect("rels"), &mut rels).expect("read");
    assert!(rels.contains(r#"Target="file:///tmp/linked.png" TargetMode="External""#), "{rels}");
}

/// #210: `.potx` and `.ppsx` were written with the `.pptx` main content type.
#[test]
fn templates_and_slide_shows_declare_their_own_main_content_type() {
    let p = rich_deck();
    for (ext, kind) in [("pptx", "presentation"), ("potx", "template"), ("ppsx", "slideshow")] {
        let bytes = deckcraft_pptx::export_as(&p, ext).expect("export");
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).expect("zip");
        let ct = std::io::read_to_string(z.by_name("[Content_Types].xml").expect("content types")).expect("read");
        assert!(ct.contains(&format!("presentationml.{kind}.main+xml")), "{ext}: {ct}");
        assert_eq!(deckcraft_pptx::import(&bytes).expect("import").slides.len(), p.slides.len(), "{ext} reads back");
    }
}

/// #214: a lone guide was always written `adj`, but these presets name it `adj1`, and PowerPoint
/// won't open the file otherwise. (Names as PowerPoint writes them.)
#[test]
fn single_guide_names_follow_the_preset() {
    let mut p = defaults::new_presentation(None);
    let mut s = new_slide(&mut p, "Guides");
    let cases = [
        ("bentConnector3", "adj1"),
        ("curvedConnector3", "adj1"),
        ("frame", "adj1"),
        ("mathPlus", "adj1"),
        ("mathMinus", "adj1"),
        ("mathMultiply", "adj1"),
        ("roundRect", "adj"),
        ("chevron", "adj"),
    ];
    for (prst, _) in cases {
        let mut sh = shape(&mut p, prst, 0.0, 0.0, 100.0, 50.0);
        sh.geom = Geom::Preset { name: prst.into(), adj: vec![20000.0] };
        s.shapes.push(sh);
    }
    p.slides.push(Arc::new(s));
    let bytes = deckcraft_pptx::export(&p).expect("export");
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).expect("zip");
    let xml = std::io::read_to_string(z.by_name("ppt/slides/slide2.xml").expect("slide")).expect("read");
    for (prst, gd) in cases {
        assert!(xml.contains(&format!(r#"<a:prstGeom prst="{prst}"><a:avLst><a:gd name="{gd}" fmla="val 20000"/>"#)), "{prst} → {gd}: {xml}");
    }
}

/// Large finite guides position a visible bend even when the connector is only one EMU wide.
#[test]
fn large_connector_adjustments_keep_their_elbows_through_two_saves() {
    let cases = [
        ("ordinary", 80.0, 50000.0),
        ("near control", 1588.0 / 12700.0, -12800000.0),
        ("near negative", 1.0 / 12700.0, -20320000000.0),
        ("near positive", 1.0 / 12700.0, 20320000000.0),
        ("zero width", 0.0, 20320000000.0),
        ("ordinary negative", 80.0, -20000.0),
    ];
    let mut p = defaults::new_presentation(None);
    p.slides.clear();
    let mut s = new_slide(&mut p, "Connectors");
    s.shapes.clear();
    for (name, width, guide) in cases {
        let mut sh = shape(&mut p, name, 100.0, 80.0, width, 140.0);
        sh.kind = ShapeKind::Connector { start: None, end: None };
        sh.geom = Geom::Preset { name: "bentConnector3".into(), adj: vec![guide] };
        s.shapes.push(sh);
    }
    p.slides.push(Arc::new(s));
    for generation in 1..=2 {
        p = deckcraft_pptx::import(&deckcraft_pptx::export(&p).expect("export")).expect("reopen");
        for (name, width, guide) in cases {
            let sh = p.slides[0].shapes.iter().find(|sh| sh.name == name).expect("connector");
            let Geom::Preset { name: preset, adj } = &sh.geom else {
                panic!("preset connector");
            };
            assert_eq!(adj, &[guide], "{name}, save {generation}");
            let xfrm = sh.xfrm.expect("transform");
            assert_eq!((xfrm.w, xfrm.h), (width, 140.0));
            let expected = deckcraft_geom::preset::build("bentConnector3", width, 140.0, &[guide]).expect("expected geometry");
            let actual = deckcraft_geom::preset::build(preset, xfrm.w, xfrm.h, adj).expect("reopened geometry");
            assert_eq!(actual.outline(), expected.outline(), "{name}, save {generation}");
        }
    }
}

/// #147: PowerPoint's Float Down is preset 47 (Float Up is 42); it imported as a plain fade.
#[test]
fn float_down_is_preset_47_both_ways() {
    let mut p = defaults::new_presentation(None);
    let mut s = new_slide(&mut p, "Float");
    let sh = shape(&mut p, "Building", 0.0, 0.0, 100.0, 100.0);
    let id = sh.id;
    s.shapes.push(sh);
    s.animations = vec![
        Animation { shape: id, class: AnimClass::Entrance, effect: "float".into(), option: "d".into(), ..Default::default() },
        Animation { shape: id, class: AnimClass::Exit, effect: "floatOut".into(), option: "d".into(), ..Default::default() },
        Animation { shape: id, class: AnimClass::Exit, effect: "floatOut".into(), option: "u".into(), ..Default::default() },
    ];
    p.slides.push(Arc::new(s));
    let bytes = deckcraft_pptx::export(&p).expect("export");
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).expect("zip");
    let xml = std::io::read_to_string(z.by_name("ppt/slides/slide2.xml").expect("slide")).expect("read");
    assert!(xml.contains(r#"presetID="47" presetClass="entr" presetSubtype="0""#), "{xml}");
    assert!(xml.contains(r#"presetID="47" presetClass="exit" presetSubtype="0""#), "{xml}");
    assert!(xml.contains(r#"presetID="42" presetClass="exit" presetSubtype="0""#), "{xml}");
    let q = deckcraft_pptx::import(&bytes).expect("import");
    let got: Vec<(&str, &str)> = q.slides[1].animations.iter().map(|a| (a.effect.as_str(), a.option.as_str())).collect();
    assert_eq!(got, [("float", "d"), ("floatOut", "d"), ("floatOut", "u")]);
}

#[test]
fn embedded_fonts_round_trip() {
    let mut p = Presentation::default();
    let regular = p.add_media("font1.fntdata", "application/x-fontdata", b"REGULAR-FONT-BYTES-0123456789".to_vec()).expect("media");
    let bold = p.add_media("font2.fntdata", "application/x-fontdata", b"BOLD-FONT-BYTES-0123456789abcd".to_vec()).expect("media");
    p.embedded_fonts.push(deckcraft_model::EmbeddedFont {
        typeface: "Deck Test Face".into(),
        panose: Some("020B0604020202020204".into()),
        pitch_family: Some(34),
        charset: Some(0),
        regular: Some(regular),
        bold: Some(bold),
        italic: Some(regular),
        bold_italic: None,
    });
    // The same typeface again is one entry: PowerPoint requires each typeface once.
    p.embedded_fonts.push(deckcraft_model::EmbeddedFont { typeface: "Deck Test Face".into(), regular: Some(bold), ..Default::default() });
    let bytes = deckcraft_pptx::export(&p).expect("export");
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes.clone())).expect("zip");
    let mut xml = String::new();
    std::io::Read::read_to_string(&mut zip.by_name("ppt/presentation.xml").expect("pres"), &mut xml).expect("read");
    assert!(xml.contains("embedTrueTypeFonts=\"1\""));
    let notes = xml.find("<p:notesSz").expect("notesSz");
    let list = xml.find("<p:embeddedFontLst>").expect("list");
    let dts = xml.find("<p:defaultTextStyle").expect("style");
    assert!(notes < list && list < dts);
    assert!(xml.contains(r#"<p:font typeface="Deck Test Face" panose="020B0604020202020204" pitchFamily="34" charset="0"/>"#));
    assert_eq!(xml.matches("<p:embeddedFont>").count(), 1);
    let mut rels = String::new();
    std::io::Read::read_to_string(&mut zip.by_name("ppt/_rels/presentation.xml.rels").expect("rels"), &mut rels).expect("read");
    assert_eq!(rels.matches("relationships/font").count(), 2, "regular and italic share one part");
    let q = deckcraft_pptx::import(&bytes).expect("import");
    assert_eq!(q.embedded_fonts.len(), 1);
    let f = &q.embedded_fonts[0];
    assert_eq!(f.typeface, "Deck Test Face");
    assert_eq!(f.panose.as_deref(), Some("020B0604020202020204"));
    assert_eq!(f.pitch_family, Some(34));
    assert_eq!(f.charset, Some(0));
    assert_eq!(q.media(f.regular.expect("regular")).expect("media").data.as_slice(), b"REGULAR-FONT-BYTES-0123456789");
    assert_eq!(q.media(f.bold.expect("bold")).expect("media").data.as_slice(), b"BOLD-FONT-BYTES-0123456789abcd");
    assert_eq!(f.italic, f.regular);
    assert!(f.bold_italic.is_none());
}

/// A package whose font list this writer did not produce: bold only, relationship id not `rIdN`.
#[test]
fn an_embedded_font_list_already_in_the_package_is_read() {
    let bytes = deckcraft_pptx::export(&Presentation::default()).expect("export");
    let font = b"FACE-BYTES-FROM-THE-PACKAGE";
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("zip");
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).expect("entry");
        let name = f.name().to_string();
        let mut data = vec![];
        std::io::Read::read_to_end(&mut f, &mut data).expect("read");
        if name == "ppt/presentation.xml" {
            let xml = String::from_utf8(data).expect("utf8");
            let xml = xml.replacen(
                "<p:defaultTextStyle",
                concat!(
                    r#"<p:embeddedFontLst><p:embeddedFont><p:font typeface="Package Face" pitchFamily="2"/>"#,
                    r#"<p:bold r:id="rIdFont"/></p:embeddedFont></p:embeddedFontLst><p:defaultTextStyle"#
                ),
                1,
            );
            data = xml.into_bytes();
        } else if name == "ppt/_rels/presentation.xml.rels" {
            let xml = String::from_utf8(data).expect("utf8");
            let xml = xml.replacen(
                "</Relationships>",
                r#"<Relationship Id="rIdFont" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/font" Target="fonts/font1.fntdata"/></Relationships>"#,
                1,
            );
            data = xml.into_bytes();
        } else if name == "[Content_Types].xml" {
            let xml = String::from_utf8(data).expect("utf8");
            let xml = xml.replacen("</Types>", r#"<Default Extension="fntdata" ContentType="application/x-fontdata"/></Types>"#, 1);
            data = xml.into_bytes();
        }
        out.start_file(name, zip::write::SimpleFileOptions::default()).expect("start");
        std::io::Write::write_all(&mut out, &data).expect("write");
    }
    out.start_file("ppt/fonts/font1.fntdata", zip::write::SimpleFileOptions::default()).expect("font");
    std::io::Write::write_all(&mut out, font).expect("font bytes");
    let patched = out.finish().expect("finish").into_inner();
    let p = deckcraft_pptx::import(&patched).expect("import");
    let f = p.embedded_fonts.first().expect("font");
    assert_eq!(f.typeface, "Package Face");
    assert_eq!(f.pitch_family, Some(2));
    assert!(f.panose.is_none() && f.regular.is_none() && f.italic.is_none() && f.bold_italic.is_none());
    assert_eq!(p.media(f.bold.expect("bold")).expect("media").data.as_slice(), font);
}

/// Text transparency (#32) is `a:alpha` on the run's colour, and survives a save.
#[test]
fn text_transparency_round_trips() {
    use deckcraft_engine::Session;
    let mut s = Session::with_new();
    s.execute("shape.insert", &serde_json::json!({"preset": "rect", "rect": [100, 100, 200, 100], "text": "Hi"})).unwrap();
    s.execute("format.color", &serde_json::json!({"color": "#336699"})).unwrap();
    s.execute("format.textTransparency", &serde_json::json!({"value": 0.4})).unwrap();
    let doc = s.doc().unwrap().doc.clone();
    let bytes = deckcraft_pptx::export(&doc).unwrap();
    let back = deckcraft_pptx::import(&bytes).unwrap();
    let sh = back.slides[0].shapes.iter().find(|sh| sh.text.as_ref().is_some_and(|t| t.text().contains("Hi"))).unwrap();
    let Some(deckcraft_model::Fill::Solid { color }) = &sh.text.as_ref().unwrap().paragraphs[0].runs[0].props.fill else { panic!() };
    assert!((color.alpha() - 0.6).abs() < 1e-9, "{color:?}");
}

/// A tiny 8 kHz mono 16-bit WAV of `n` silent samples.
fn wav(n: u32) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + n * 2).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&8000u32.to_le_bytes());
    v.extend_from_slice(&16000u32.to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(n * 2).to_le_bytes());
    v.resize(v.len() + n as usize * 2, 0);
    v
}

/// Transitions › Sound: a chosen sound is embedded, saved as `p:sndAc`, read back, and No Sound
/// removes it; a file that isn't audio is refused.
#[test]
fn transition_sound_is_set_saved_and_removed() {
    use deckcraft_engine::Session;
    use serde_json::json;
    let b64 = deckcraft_engine::cmd::base64_encode(&wav(800));
    let mut s = Session::with_new();
    s.execute("transition.set", &json!({"kind": "fade"})).unwrap();
    let r = s.execute("transition.sound", &json!({"data": b64, "name": "chime.wav"})).unwrap();
    assert_eq!(r["playable"], true, "{r}");
    let doc = s.doc().unwrap().doc.clone();
    let m = doc.slides[0].transition.as_ref().and_then(|t| t.sound).expect("sound set");
    assert_eq!(doc.media(m).map(|i| i.name.as_str()), Some("chime.wav"));
    let bytes = deckcraft_pptx::export(&doc).unwrap();
    let back = deckcraft_pptx::import(&bytes).unwrap();
    let t = back.slides[0].transition.as_ref().expect("transition");
    assert_eq!(t.kind, "fade");
    let m = t.sound.expect("sound survives the save");
    assert_eq!(back.media(m).map(|i| i.data.len()), Some(wav(800).len()));
    s.execute("transition.sound", &json!({"none": true})).unwrap();
    assert!(s.doc().unwrap().doc.slides[0].transition.as_ref().is_some_and(|t| t.sound.is_none()));
    let not_audio = deckcraft_engine::cmd::base64_encode(b"hello, not a sound");
    assert!(s.execute("transition.sound", &json!({"data": not_audio, "name": "notes.txt"})).is_err());
}

/// Built-in transition sounds, Loop Until Next Sound and [Stop Previous Sound] are set, saved as
/// PowerPoint writes them (`p:stSnd loop="1"`, `p:endSnd`) and read back.
#[test]
fn transition_sound_options_round_trip() {
    use deckcraft_engine::Session;
    use serde_json::json;
    let mut s = Session::with_new();
    s.execute("slide.new", &json!({})).unwrap();
    s.execute("slide.new", &json!({})).unwrap();
    s.execute("transition.sound", &json!({"builtin": "chime", "loop": true, "index": 0})).unwrap();
    s.execute("transition.sound", &json!({"builtin": "Chime", "index": 1})).unwrap();
    s.execute("transition.sound", &json!({"stopPrevious": true, "index": 2})).unwrap();
    let doc = s.doc().unwrap().doc.clone();
    assert_eq!(doc.media.len(), 1, "the same built-in sound is embedded once");
    assert_eq!(doc.media[0].name, "Chime.wav");
    let bytes = deckcraft_pptx::export(&doc).unwrap();
    let slide1 = {
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        let mut x = String::new();
        std::io::Read::read_to_string(&mut z.by_name("ppt/slides/slide1.xml").unwrap(), &mut x).unwrap();
        x
    };
    assert!(slide1.contains(r#"<p:stSnd loop="1">"#) && slide1.contains(r#"name="Chime.wav""#), "{slide1}");
    let back = deckcraft_pptx::import(&bytes).unwrap();
    let t: Vec<_> = back.slides.iter().map(|s| s.transition.clone().unwrap_or_default()).collect();
    assert!(t[0].sound.is_some() && t[0].sound_loop && !t[0].stop_sound);
    assert!(t[1].sound.is_some() && !t[1].sound_loop);
    assert!(t[2].sound.is_none() && t[2].stop_sound, "{:?}", t[2]);
    // Turning the loop off alone keeps the sound.
    s.execute("transition.sound", &json!({"loop": false, "index": 0})).unwrap();
    let t0 = s.doc().unwrap().doc.slides[0].transition.clone().unwrap();
    assert!(t0.sound.is_some() && !t0.sound_loop);
    assert!(s.execute("transition.sound", &json!({"builtin": "foghorn"})).is_err());
}

/// Replace `from` with `to` in every slide part of an exported deck.
fn patch_slides(bytes: Vec<u8>, from: &str, to: &str) -> Vec<u8> {
    use std::io::{Read, Write};
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("zip");
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for i in 0..z.len() {
        let mut f = z.by_index(i).expect("entry");
        let name = f.name().to_string();
        let mut data = vec![];
        f.read_to_end(&mut data).expect("read");
        if name.starts_with("ppt/slides/slide") && name.ends_with(".xml") {
            data = String::from_utf8(data).expect("utf8").replace(from, to).into_bytes();
        }
        out.start_file(name, zip::write::SimpleFileOptions::default()).expect("start");
        out.write_all(&data).expect("write");
    }
    out.finish().expect("finish").into_inner()
}

fn runs_of(p: &Presentation, needle: &str) -> Vec<(RunKind, String)> {
    for s in &p.slides {
        for sh in &s.shapes {
            let Some(t) = &sh.text else { continue };
            for para in &t.paragraphs {
                if para.runs.iter().any(|r| r.text.contains(needle)) {
                    return para.runs.iter().map(|r| (r.kind.clone(), r.text.clone())).collect();
                }
            }
        }
    }
    panic!("no paragraph with {needle}");
}

#[test]
fn literal_line_ends_in_run_text_read_as_breaks_and_survive_save() {
    let mut p = defaults::new_presentation(None);
    let mut s = shape(&mut p, "Code", 10.0, 10.0, 400.0, 100.0);
    s.text = Some(TextBody::from_text("CRLFHERE"));
    let first = p.slides[0].id;
    p.slide_mut(first).expect("slide").shapes.push(s);
    let bytes = patch_slides(deckcraft_pptx::export(&p).expect("export"), "CRLFHERE", "one\r\n   two\rthree");
    let a = deckcraft_pptx::import(&bytes).expect("import");
    let want = vec![
        (RunKind::Text, "one".to_string()),
        (RunKind::Break, String::new()),
        (RunKind::Text, "   two".to_string()),
        (RunKind::Break, String::new()),
        (RunKind::Text, "three".to_string()),
    ];
    assert_eq!(runs_of(&a, "one"), want);
    assert_eq!(runs_of(&round(&a), "one"), want, "save and reopen keep the runs");
}

#[test]
fn alternate_run_language_survives_pptx_saves_and_absence() {
    let mut p = defaults::new_presentation(None);
    let first = p.slides[0].id;
    let slide = p.slide_mut(first).expect("slide");
    slide.shapes[0].text = Some(TextBody {
        paragraphs: vec![Paragraph {
            runs: vec![
                Run::with("bonjour", RunProps { lang: Some("en-US".into()), alt_lang: Some("fr-FR".into()), ..Default::default() }),
                Run::with(" plain", RunProps { lang: Some("en-US".into()), ..Default::default() }),
            ],
            ..Default::default()
        }],
        ..Default::default()
    });
    let mut q = round(&p);
    for _ in 0..2 {
        let runs = &q.slides[0].shapes[0].text.as_ref().expect("text").paragraphs[0].runs;
        assert_eq!(runs[0].props.lang.as_deref(), Some("en-US"));
        assert_eq!(runs[0].props.alt_lang.as_deref(), Some("fr-FR"));
        assert_eq!(runs[1].props.lang.as_deref(), Some("en-US"));
        assert_eq!(runs[1].props.alt_lang, None);
        q = round(&q);
    }
}

#[test]
fn color_pulse_keeps_its_effect_colour_and_duration() {
    let mut p = defaults::new_presentation(None);
    let s = shape(&mut p, "Pulsed", 10.0, 10.0, 200.0, 100.0);
    let id = s.id;
    let first = p.slides[0].id;
    let sl = p.slide_mut(first).expect("slide");
    sl.shapes.push(s);
    let white = ColorRef::rgb(Rgba::rgb(255, 255, 255));
    sl.animations.push(Animation {
        shape: id,
        class: AnimClass::Emphasis,
        effect: "colorPulse".into(),
        duration_ms: 1000,
        color: Some(white.clone()),
        ..Default::default()
    });
    let q = round(&p);
    let a = &q.slides[0].animations[0];
    assert_eq!((a.class, a.effect.as_str()), (AnimClass::Emphasis, "colorPulse"));
    assert_eq!(a.color, Some(white));
    assert_eq!(a.amount, None, "no grow/shrink scale is invented");
    assert_eq!(a.duration_ms, 1000);
}

#[test]
fn notes_master_placeholders_and_colour_map_survive_save() {
    let mut p = defaults::new_presentation(None);
    let mut box1 = shape(&mut p, "Confidential", 100000.0, 200000.0, 3000000.0, 400000.0);
    box1.text = Some(TextBody::from_text("Company confidential"));
    let map: Vec<(String, String)> = [("bg1", "dk1"), ("tx1", "lt1"), ("bg2", "dk2"), ("tx2", "lt2")]
        .iter()
        .chain(
            [
                ("accent1", "accent1"),
                ("accent2", "accent2"),
                ("accent3", "accent3"),
                ("accent4", "accent4"),
                ("accent5", "accent5"),
                ("accent6", "accent6"),
                ("hlink", "hlink"),
                ("folHlink", "folHlink"),
            ]
            .iter(),
        )
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    let m = deckcraft_model::Master { name: "Notes Master".into(), shapes: vec![box1], color_map: map.clone(), ..Default::default() };
    p.notes_master = Some(Arc::new(m));
    // No slide has notes: the notes master must still be written.
    let q = round(&p);
    let nm = q.notes_master.as_deref().expect("notes master kept");
    assert_eq!(nm.shapes.len(), 1, "own shapes, not the defaults");
    let sh = &nm.shapes[0];
    assert_eq!(sh.name, "Confidential");
    assert_eq!(sh.text.as_ref().map(|t| t.text()), Some("Company confidential".to_string()));
    assert_eq!(nm.color_map, map);
}

/// A part's bytes, if the package has it.
fn pkg_part(pkg: &[u8], name: &str) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(pkg)).unwrap();
    let mut f = z.by_name(name).ok()?;
    let mut v = vec![];
    f.read_to_end(&mut v).unwrap();
    Some(v)
}

/// Rewrite a package's parts with `edit`, then add `extra` parts.
fn repack(pkg: &[u8], edit: impl Fn(&str, String) -> String, extra: &[(&str, &[u8])]) -> Vec<u8> {
    use std::io::{Read, Write};
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(pkg)).unwrap();
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();
    for i in 0..z.len() {
        let mut f = z.by_index(i).unwrap();
        let name = f.name().to_string();
        let mut data = vec![];
        f.read_to_end(&mut data).unwrap();
        if name.ends_with(".xml") || name.ends_with(".rels") {
            data = edit(&name, String::from_utf8(data).unwrap()).into_bytes();
        }
        out.start_file(name, opts).unwrap();
        out.write_all(&data).unwrap();
    }
    for (name, data) in extra {
        out.start_file(*name, opts).unwrap();
        out.write_all(data).unwrap();
    }
    out.finish().unwrap().into_inner()
}

const WATERFALL_CX: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cx:chartSpace xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:cx="http://schemas.microsoft.com/office/drawing/2014/chartex"><cx:chartData><cx:externalData r:id="rId1" cx:autoUpdate="0"/><cx:data id="0"><cx:strDim type="cat"><cx:f>Sheet1!$A$2:$A$5</cx:f><cx:lvl ptCount="4"><cx:pt idx="0">Start</cx:pt><cx:pt idx="1">Sales</cx:pt><cx:pt idx="2">Costs</cx:pt><cx:pt idx="3">End</cx:pt></cx:lvl></cx:strDim><cx:numDim type="val"><cx:f>Sheet1!$B$2:$B$5</cx:f><cx:lvl ptCount="4" formatCode="General"><cx:pt idx="0">100</cx:pt><cx:pt idx="1">40</cx:pt><cx:pt idx="2">-25</cx:pt><cx:pt idx="3">115</cx:pt></cx:lvl></cx:numDim></cx:data></cx:chartData><cx:chart><cx:title pos="t" align="ctr" overlay="0"><cx:tx><cx:txData><cx:v>Cash flow</cx:v></cx:txData></cx:tx></cx:title><cx:plotArea><cx:plotAreaRegion><cx:series layoutId="waterfall" uniqueId="{1}"><cx:tx><cx:txData><cx:f>Sheet1!$B$1</cx:f><cx:v>Amount</cx:v></cx:txData></cx:tx><cx:dataLabels pos="outEnd"><cx:visibility seriesName="0" categoryName="0" value="1"/></cx:dataLabels><cx:dataId val="0"/><cx:layoutPr><cx:subtotals><cx:idx val="0"/><cx:idx val="3"/></cx:subtotals></cx:layoutPr></cx:series></cx:plotAreaRegion><cx:axis id="0"><cx:catScaling gapWidth="0.5"/><cx:tickLabels/></cx:axis><cx:axis id="1"><cx:valScaling/><cx:majorGridlines/><cx:tickLabels/></cx:axis></cx:plotArea><cx:legend pos="b" align="ctr" overlay="0"/></cx:chart></cx:chartSpace>"#;

/// A waterfall frame as PowerPoint writes it: the chart in a `cx1` choice, a text box fallback.
const WATERFALL_FRAME: &str = r#"<mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:Choice xmlns:cx1="http://schemas.microsoft.com/office/drawing/2015/9/8/chartex" Requires="cx1"><p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="90" name="Waterfall 1"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="914400" y="914400"/><a:ext cx="6096000" cy="4064000"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.microsoft.com/office/drawing/2014/chartex"><cx:chart xmlns:cx="http://schemas.microsoft.com/office/drawing/2014/chartex" r:id="rIdCx"/></a:graphicData></a:graphic></p:graphicFrame></mc:Choice><mc:Fallback><p:sp><p:nvSpPr><p:cNvPr id="90" name="Waterfall 1"/><p:cNvSpPr><a:spLocks noTextEdit="1"/></p:cNvSpPr><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="914400" y="914400"/><a:ext cx="6096000" cy="4064000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr><p:txBody><a:bodyPr/><a:p><a:r><a:t>This chart isn't available in your version of PowerPoint.</a:t></a:r></a:p></p:txBody></p:sp></mc:Fallback></mc:AlternateContent>"#;

/// Office 2016 charts (`chartEx`): a waterfall reads as a waterfall chart with its data, not as
/// the "isn't available" fallback text, and an unchanged one is written back as itself with a
/// plain chart fallback; an edited one is written as a plain chart.
#[test]
fn chartex_waterfall_reads_and_round_trips() {
    use deckcraft_engine::Session;
    let mut s = Session::with_new();
    s.execute("slide.new", &serde_json::json!({"layout": "blank"})).unwrap();
    let base = deckcraft_pptx::export(&s.doc().unwrap().doc).unwrap();
    let slide = (1..=9).map(|i| format!("ppt/slides/slide{i}.xml")).find(|n| pkg_part(&base, n).is_some()).unwrap();
    let slide_rels = slide.replace("slides/", "slides/_rels/") + ".rels";
    let cx_rels = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/package" Target="../embeddings/Microsoft_Excel_Worksheet.xlsx"/></Relationships>"#;
    let pptx = repack(
        &base,
        |name, x| {
            if name == slide {
                x.replacen("</p:spTree>", &format!("{WATERFALL_FRAME}</p:spTree>"), 1)
            } else if name == slide_rels {
                x.replace("</Relationships>", r#"<Relationship Id="rIdCx" Type="http://schemas.microsoft.com/office/2014/relationships/chartEx" Target="../charts/chartEx1.xml"/></Relationships>"#)
            } else if name == "[Content_Types].xml" {
                x.replace(
                    "</Types>",
                    r#"<Override PartName="/ppt/charts/chartEx1.xml" ContentType="application/vnd.ms-office.chartex+xml"/></Types>"#,
                )
            } else {
                x
            }
        },
        &[("ppt/charts/chartEx1.xml", WATERFALL_CX.as_bytes()), ("ppt/charts/_rels/chartEx1.xml.rels", cx_rels.as_bytes())],
    );
    let doc = deckcraft_pptx::import(&pptx).unwrap();
    let chart = |d: &Presentation| -> Option<Chart> {
        d.slides.iter().flat_map(|s| s.shapes.iter()).find_map(|sh| match &sh.kind {
            ShapeKind::Chart(c) => Some((**c).clone()),
            _ => None,
        })
    };
    let c = chart(&doc).expect("the waterfall is a chart, not the fallback text");
    assert_eq!(c.kind, ChartType::Waterfall);
    assert_eq!(c.title.as_deref(), Some("Cash flow"));
    assert_eq!(c.categories, ["Start", "Sales", "Costs", "End"]);
    assert_eq!(c.series.len(), 1);
    assert_eq!(c.series[0].name, "Amount");
    assert_eq!(c.series[0].values, [Some(100.0), Some(40.0), Some(-25.0), Some(115.0)]);
    assert!(c.data_labels);
    assert_eq!(c.legend.as_deref(), Some("b"));
    let no_fallback_text = doc.slides.iter().flat_map(|s| s.shapes.iter()).all(|sh| !format!("{:?}", sh.kind).contains("isn't available"));
    assert!(no_fallback_text);

    // Unchanged: chartEx back in a cx1 choice, a plain chart as fallback, no workbook link.
    let out = deckcraft_pptx::export(&doc).unwrap();
    let ex = String::from_utf8(pkg_part(&out, "ppt/charts/chartEx1.xml").expect("chartEx part")).unwrap();
    assert!(ex.contains(r#"layoutId="waterfall""#) && !ex.contains("externalData"), "{ex}");
    let types = String::from_utf8(pkg_part(&out, "[Content_Types].xml").unwrap()).unwrap();
    assert!(types.contains("application/vnd.ms-office.chartex+xml"));
    let sx = String::from_utf8(pkg_part(&out, &slide).unwrap()).unwrap();
    assert!(sx.contains(r#"Requires="cx1""#) && sx.contains("mc:Fallback") && sx.contains("c:chart "), "{sx}");
    let srels = String::from_utf8(pkg_part(&out, &slide_rels).unwrap()).unwrap();
    assert!(srels.contains("relationships/chartEx") && srels.contains("relationships/chart\""), "{srels}");
    assert!(pkg_part(&out, "ppt/charts/chart1.xml").is_some());
    let again = deckcraft_pptx::import(&out).unwrap();
    assert_eq!(chart(&again).map(|c| (c.kind, c.categories, c.series.len())), Some((ChartType::Waterfall, c.categories.clone(), 1)));

    // Edited: the kept part no longer matches, so a chartEx part is written from the model,
    // still a waterfall with the new value.
    let mut edited = doc.clone();
    for sl in &mut edited.slides {
        for sh in &mut Arc::make_mut(sl).shapes {
            if let ShapeKind::Chart(ch) = &mut sh.kind {
                ch.series[0].values[1] = Some(55.0);
            }
        }
    }
    let out = deckcraft_pptx::export(&edited).unwrap();
    let ex = String::from_utf8(pkg_part(&out, "ppt/charts/chartEx1.xml").expect("a written chartEx part")).unwrap();
    assert!(ex.contains(r#"layoutId="waterfall""#), "{ex}");
    assert!(String::from_utf8(pkg_part(&out, &slide).unwrap()).unwrap().contains("cx1"));
    let again = chart(&deckcraft_pptx::import(&out).unwrap()).unwrap();
    assert_eq!(again.kind, ChartType::Waterfall);
    assert_eq!(again.series[0].values[1], Some(55.0));
    assert_eq!(again.subtotals, c.subtotals);
}

/// New Office 2016 charts made in DeckCraft are written as chartEx parts that read back as the
/// same type, data and options.
#[test]
fn chartex_new_charts_save_as_their_own_type() {
    use deckcraft_engine::Session;
    for (ty, kind) in [
        ("waterfall", ChartType::Waterfall),
        ("funnel", ChartType::Funnel),
        ("treemap", ChartType::Treemap),
        ("sunburst", ChartType::Sunburst),
        ("histogram", ChartType::Histogram),
        ("pareto", ChartType::Pareto),
        ("boxwhisker", ChartType::BoxWhisker),
    ] {
        let chart = |d: &Presentation| -> Option<Chart> {
            d.slides.iter().flat_map(|s| s.shapes.iter()).find_map(|sh| match &sh.kind {
                ShapeKind::Chart(c) => Some((**c).clone()),
                _ => None,
            })
        };
        let mut s = Session::with_new();
        s.execute("slide.new", &serde_json::json!({"layout": "blank"})).unwrap();
        s.execute("insert.chart", &serde_json::json!({"type": ty})).unwrap();
        let id = s.doc().unwrap().selection.shapes[0].0;
        let opts = match kind {
            ChartType::Histogram | ChartType::Pareto => serde_json::json!({"id": id, "binCount": 4, "title": "T"}),
            ChartType::Waterfall => serde_json::json!({"id": id, "subtotals": [3], "title": "T"}),
            _ => serde_json::json!({"id": id, "title": "T"}),
        };
        s.execute("chart.options", &opts).unwrap();
        let doc = &s.doc().unwrap().doc;
        let before = chart(doc).unwrap();
        let out = deckcraft_pptx::export(doc).unwrap();
        assert!(pkg_part(&out, "ppt/charts/chartEx1.xml").is_some(), "{ty}: no chartEx part");
        assert!(pkg_part(&out, "ppt/charts/chart1.xml").is_some(), "{ty}: no fallback chart");
        let after = chart(&deckcraft_pptx::import(&out).unwrap()).unwrap();
        assert_eq!(after.kind, kind, "{ty}");
        assert_eq!(after.title.as_deref(), Some("T"), "{ty}");
        assert_eq!(after.categories, before.categories, "{ty}");
        let n = if matches!(kind, ChartType::BoxWhisker) { before.series.len() } else { 1 };
        assert_eq!(after.series.len(), n, "{ty}");
        assert_eq!(after.series[0].values, before.series[0].values, "{ty}");
        assert_eq!((after.bin_count, after.subtotals.clone()), (before.bin_count, before.subtotals.clone()), "{ty}");
    }
}

/// A two-level treemap keeps its parent level, and a box chart its inclusive quartiles, through
/// a save and reopen.
#[test]
fn chartex_levels_and_quartiles_round_trip() {
    use deckcraft_engine::Session;
    let chart = |d: &Presentation| -> Option<Chart> {
        d.slides.iter().flat_map(|s| s.shapes.iter()).find_map(|sh| match &sh.kind {
            ShapeKind::Chart(c) => Some((**c).clone()),
            _ => None,
        })
    };
    for ty in ["treemap", "sunburst", "boxwhisker"] {
        let mut s = Session::with_new();
        s.execute("slide.new", &serde_json::json!({"layout": "blank"})).unwrap();
        s.execute("insert.chart", &serde_json::json!({"type": ty})).unwrap();
        let id = s.doc().unwrap().selection.shapes[0].0;
        if ty == "boxwhisker" {
            s.execute("chart.options", &serde_json::json!({"id": id, "quartiles": "inclusive"})).unwrap();
        } else {
            s.execute(
                "chart.data",
                &serde_json::json!({"id": id, "levels": [["Q1", "", "", "Q2", "", "", "Q3", ""], ["H1", "", "", "", "", "", "H2", ""]]}),
            )
            .unwrap();
        }
        let before = chart(&s.doc().unwrap().doc).unwrap();
        let out = deckcraft_pptx::export(&s.doc().unwrap().doc).unwrap();
        let after = chart(&deckcraft_pptx::import(&out).unwrap()).unwrap();
        assert_eq!(after.category_levels, before.category_levels, "{ty}");
        assert_eq!(after.inclusive_quartiles, before.inclusive_quartiles, "{ty}");
        assert_eq!(after.category_tree(), before.category_tree(), "{ty}");
        // Unchanged since import, the part is written back as read.
        let again = deckcraft_pptx::export(&deckcraft_pptx::import(&out).unwrap()).unwrap();
        assert_eq!(chart(&deckcraft_pptx::import(&again).unwrap()).unwrap().category_levels, before.category_levels, "{ty}");
    }
}

/// Radar, stock (with and without up/down bars) and combo charts keep their own plot groups
/// through a save and reopen, written as PowerPoint writes them.
#[test]
fn radar_stock_and_combo_round_trip() {
    use deckcraft_engine::Session;
    let chart = |d: &Presentation| -> Option<Chart> {
        d.slides.iter().flat_map(|s| s.shapes.iter()).find_map(|sh| match &sh.kind {
            ShapeKind::Chart(c) => Some((**c).clone()),
            _ => None,
        })
    };
    for (ty, bars) in [("radar", false), ("filledradar", false), ("stock", false), ("stock", true), ("combo", false)] {
        let mut s = Session::with_new();
        s.execute("slide.new", &serde_json::json!({"layout": "blank"})).unwrap();
        s.execute("insert.chart", &serde_json::json!({"type": ty})).unwrap();
        let id = s.doc().unwrap().selection.shapes[0].0;
        if bars {
            s.execute("chart.options", &serde_json::json!({"id": id, "upDownBars": true})).unwrap();
        }
        let before = chart(&s.doc().unwrap().doc).unwrap();
        let out = deckcraft_pptx::export(&s.doc().unwrap().doc).unwrap();
        let after = chart(&deckcraft_pptx::import(&out).unwrap()).unwrap();
        assert_eq!(after.kind, before.kind, "{ty}");
        assert_eq!(after.up_down_bars, bars, "{ty}");
        assert_eq!(after.series.len(), before.series.len(), "{ty}");
        let kinds: Vec<_> = after.series.iter().map(|x| after.series_kind(x)).collect();
        let want: Vec<_> = before.series.iter().map(|x| before.series_kind(x)).collect();
        assert_eq!(kinds, want, "{ty}");
        for (a, b) in after.series.iter().zip(&before.series) {
            assert_eq!(a.values, b.values, "{ty}");
        }
    }
}

/// Home › New Slide › Reuse Slides: slides from another .pptx land after the current slide with
/// their pictures (fresh media ids, same bytes) and animations on the renumbered shapes. By
/// default they take a destination layout; Keep Source Formatting brings the source master.
#[test]
fn reuse_slides_from_another_presentation() {
    use deckcraft_engine::Session;
    use serde_json::json;
    let b64 = deckcraft_engine::cmd::base64_encode;
    // Source: three slides, a picture with an animation on the second.
    let mut src = Session::with_new();
    src.execute("slide.new", &json!({})).unwrap();
    let pic = png(8, 8, [10, 200, 30]);
    src.execute("insert.picture", &json!({"data": b64(&pic), "name": "green.png"})).unwrap();
    src.execute("animation.add", &json!({"effect": "fade"})).unwrap();
    src.execute("slide.new", &json!({})).unwrap();
    let src_bytes = deckcraft_pptx::export(&src.doc().unwrap().doc).unwrap();

    let mut s = Session::with_new();
    // Media ids in the two decks overlap; the destination's own picture must stay its own.
    let red = png(4, 4, [200, 0, 0]);
    s.execute("insert.picture", &json!({"data": b64(&red), "name": "red.png"})).unwrap();
    let r = s.execute("slide.reuse", &json!({"data": b64(&src_bytes), "name": "src.pptx", "slides": [1]})).unwrap();
    assert_eq!(r["slides"], 1, "{r}");
    let doc = s.doc().unwrap().doc.clone();
    assert_eq!(doc.slides.len(), 2);
    assert_eq!(s.doc().unwrap().selection.slide, 1, "the reused slide is selected");
    let pictures = |sl: &deckcraft_model::Slide| {
        let mut v = vec![];
        deckcraft_model::walk(&sl.shapes, &mut |sh, _| {
            if let deckcraft_model::ShapeKind::Picture { fill } = &sh.kind {
                v.push(fill.media);
            }
        });
        v
    };
    let mine = pictures(&doc.slides[0]);
    let reused = pictures(&doc.slides[1]);
    assert_eq!(doc.media(mine[0]).map(|m| m.data.as_slice() == red.as_slice()), Some(true), "own picture untouched");
    assert_eq!(doc.media(reused[0]).map(|m| m.data.as_slice() == pic.as_slice()), Some(true), "reused picture copied");
    let anim = &doc.slides[1].animations[0];
    assert!(doc.slides[1].shapes.iter().any(|sh| sh.id == anim.shape), "animation follows its shape");
    assert!(doc.layout(doc.slides[1].layout).is_some(), "uses a destination layout");
    assert_eq!(doc.masters.len(), 1);
    // The result saves and reopens.
    let back = deckcraft_pptx::import(&deckcraft_pptx::export(&doc).unwrap()).unwrap();
    assert_eq!(back.slides.len(), 2);

    // Keep Source Formatting: every slide, and the source master comes along.
    let r = s.execute("slide.reuse", &json!({"data": b64(&src_bytes), "name": "src.pptx", "keepFormatting": true, "at": 0})).unwrap();
    assert_eq!(r["slides"], 3);
    let doc = s.doc().unwrap().doc.clone();
    assert_eq!(doc.slides.len(), 5);
    assert_eq!(doc.masters.len(), 2);
    for sl in &doc.slides[..3] {
        assert_eq!(doc.layout(sl.layout).map(|(m, _)| m.id), Some(doc.masters[1].id), "source master");
    }
    // One undo removes the whole reuse.
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.doc().unwrap().doc.slides.len(), 2);
    assert!(s.execute("slide.reuse", &json!({"data": b64(&src_bytes), "name": "src.pptx", "slides": [7]})).is_err());
    assert!(s.execute("slide.reuse", &json!({"data": b64(&[0u8, 159, 146, 150]), "name": "x.bin"})).is_err());
}

/// Picture Format › Compress Pictures: a big picture shown small is downsampled, its original
/// goes when nothing else uses it, the saved file shrinks, a copy that wasn't compressed keeps
/// the original, and undo brings everything back.
#[test]
fn compress_pictures_shrinks_the_file_and_keeps_shared_originals() {
    use deckcraft_engine::Session;
    use serde_json::json;
    let b64 = deckcraft_engine::cmd::base64_encode;
    // A noisy 1600 × 1600 picture (doesn't compress away on its own).
    let big = {
        let img = image::RgbImage::from_fn(1600, 1600, |x, y| image::Rgb([(x * 31 % 251) as u8, (y * 17 % 253) as u8, ((x * y) % 255) as u8]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    };
    let mut s = Session::with_new();
    s.execute("insert.picture", &json!({"data": b64(&big), "name": "noise.png"})).unwrap();
    s.execute("shape.resize", &json!({"w": 144, "h": 144})).unwrap();
    let size_before = deckcraft_pptx::export(&s.doc().unwrap().doc).unwrap().len();
    let r = s.execute("picture.compress", &json!({"ppi": 150})).unwrap();
    assert_eq!(r["pictures"], 1, "{r}");
    let doc = s.doc().unwrap().doc.clone();
    assert_eq!(doc.media.len(), 1, "the original is gone");
    let img = image::load_from_memory(&doc.media[0].data).unwrap();
    assert_eq!((img.width(), img.height()), (300, 300), "2 in at 150 ppi");
    let size_after = deckcraft_pptx::export(&doc).unwrap().len();
    assert!(size_after * 4 < size_before, "{size_before} → {size_after}");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.doc().unwrap().doc.media[0].data.len(), big.len(), "undo restores the original");

    // A duplicate that isn't compressed still needs the original.
    s.execute("edit.duplicate", &json!({})).unwrap();
    let copy = s.doc().unwrap().selection.shapes.clone();
    let first: Vec<_> = s.doc().unwrap().doc.slides[0].shapes.iter().map(|sh| sh.id).filter(|id| !copy.contains(id)).collect();
    s.execute("picture.compress", &json!({"ppi": 150, "ids": [first.last().unwrap().0]})).unwrap();
    let doc = s.doc().unwrap().doc.clone();
    assert_eq!(doc.media.len(), 2);
    assert!(doc.media.iter().any(|m| m.data.len() == big.len()), "the copy keeps the original");
    // All pictures: the copy gets compressed too, and the original goes.
    s.execute("picture.compress", &json!({"ppi": 150, "all": true})).unwrap();
    let doc = s.doc().unwrap().doc.clone();
    assert!(doc.media.iter().all(|m| m.data.len() < big.len() / 4));
    assert!(s.execute("picture.compress", &json!({"ppi": 5, "all": true})).is_err());
}

/// #203: SmartArt we can't draw kept its XML but lost its diagram parts on save.
#[test]
fn smartart_without_a_drawing_keeps_its_parts() {
    let mut p = defaults::new_presentation(None);
    let data = p.add_media(
        "data1.xml",
        "application/vnd.openxmlformats-officedocument.drawingml.diagramData+xml",
        br#"<dgm:dataModel xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><dgm:ptLst><dgm:pt modelId="1"><dgm:t><a:p><a:r><a:t>Manager</a:t></a:r></a:p></dgm:t></dgm:pt></dgm:ptLst><dgm:extLst><a:ext uri="http://schemas.microsoft.com/office/drawing/2008/diagram"><dsp:dataModelExt xmlns:dsp="http://schemas.microsoft.com/office/drawing/2008/diagram" relId="rId6"/></a:ext></dgm:extLst></dgm:dataModel>"#.to_vec(),
    ).expect("media");
    let mut parts = vec![("rId2".to_string(), "http://schemas.openxmlformats.org/officeDocument/2006/relationships/diagramData".to_string(), data)];
    for (i, kind) in ["Layout", "QuickStyle", "Colors"].into_iter().enumerate() {
        let xml = format!(r#"<dgm:{kind} xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram"/>"#).into_bytes();
        let m = p
            .add_media(&format!("{kind}1.xml"), &format!("application/vnd.openxmlformats-officedocument.drawingml.diagram{kind}+xml"), xml)
            .expect("media");
        parts.push((format!("rId{}", i + 3), format!("http://schemas.openxmlformats.org/officeDocument/2006/relationships/diagram{kind}"), m));
    }
    let xml = r#"<p:graphicFrame xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:nvGraphicFramePr><p:cNvPr id="4" name="Diagram 3"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="914400"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/diagram"><dgm:relIds xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" r:dm="rId2" r:lo="rId3" r:qs="rId4" r:cs="rId5"/></a:graphicData></a:graphic></p:graphicFrame>"#;
    let mut s = new_slide(&mut p, "SmartArt");
    let mut frame = shape(&mut p, "Diagram 3", 0.0, 0.0, 72.0, 72.0);
    frame.kind = ShapeKind::Opaque { xml: xml.into(), preview: None, label: "SmartArt".into(), parts };
    s.shapes.push(frame);
    p.slides.push(Arc::new(s));

    // Twice: the second save starts from what the first one wrote.
    let q = round(&round(&p));
    let ShapeKind::Opaque { parts, .. } = &by_name(&q.slides[1], "Diagram 3").kind else { panic!("not kept as an object") };
    assert_eq!(parts.len(), 4, "data, layout, style and colours all kept");
    let data = q.media(parts.iter().find(|(_, k, _)| k.ends_with("/diagramData")).expect("data").2).expect("media");
    let data = String::from_utf8_lossy(&data.data);
    assert!(data.contains("Manager"), "{data}");
    assert!(!data.contains("dataModelExt"), "the link to a drawing that isn't written is dropped: {data}");
}

/// #203: a legacy OLE object (preview in the VML drawing we don't write) is saved in PowerPoint's
/// fallback form, without `spid` and with a `p:pic` preview, or PowerPoint won't open the file.
#[test]
fn legacy_ole_object_is_written_in_the_fallback_form() {
    let mut p = defaults::new_presentation(None);
    let bin = p
        .add_media("oleObject1.bin", "application/vnd.openxmlformats-officedocument.oleObject", b"\xD0\xCF\x11\xE0not-a-real-ole".to_vec())
        .expect("media");
    let prev = p.add_media("image1.png", "image/png", png(4, 3, [10, 20, 30])).expect("media");
    let xml = r#"<p:graphicFrame xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:nvGraphicFramePr><p:cNvPr id="4" name="Object 3"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="914400"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/presentationml/2006/ole"><p:oleObj spid="_x0000_s1026" name="Slide" r:id="rId4" progId="PowerPoint.Slide.8"><p:embed/></p:oleObj></a:graphicData></a:graphic></p:graphicFrame>"#;
    let parts = vec![("rId4".to_string(), "http://schemas.openxmlformats.org/officeDocument/2006/relationships/oleObject".to_string(), bin)];
    let mut s = new_slide(&mut p, "OLE");
    let mut frame = shape(&mut p, "Object 3", 0.0, 0.0, 72.0, 72.0);
    frame.kind = ShapeKind::Opaque { xml: xml.into(), preview: Some(prev), label: "Slide".into(), parts };
    s.shapes.push(frame);
    p.slides.push(Arc::new(s));

    let q = round(&round(&p));
    let ShapeKind::Opaque { xml, parts, preview, .. } = &by_name(&q.slides[1], "Object 3").kind else { panic!("not kept as an object") };
    assert!(xml.contains("<p:pic>") && !xml.contains("spid"), "{xml}");
    assert!(preview.is_some(), "the preview picture is kept");
    assert!(parts.iter().any(|(_, k, _)| k.ends_with("/oleObject")), "the embedding is kept: {parts:?}");
}

/// Insert › Zoom: slide, section and summary zooms are linked pictures of their targets, with
/// Return to Zoom on section and summary zooms; a summary zoom makes its own slide; all of it is
/// one undo step and saves as linked pictures.
#[test]
fn zoom_inserts_linked_slide_pictures() {
    use deckcraft_engine::Session;
    use deckcraft_model::text::Action;
    use serde_json::json;
    let mut s = Session::with_new();
    for t in ["Intro", "Plan", "Budget", "Risks"] {
        s.execute("slide.new", &json!({"title": t})).unwrap();
    }
    // Slides: 0 title, 1 Intro, 2 Plan, 3 Budget, 4 Risks.
    s.execute("slide.go", &json!({"index": 0})).unwrap();
    let r = s.execute("insert.zoom", &json!({"slides": [2]})).unwrap();
    assert_eq!(r["returnToZoom"], false);
    let doc = s.doc().unwrap().doc.clone();
    let zoom = doc.slides[0].shapes.iter().find(|sh| sh.name.starts_with("Slide Zoom")).expect("slide zoom");
    let link = zoom.click.as_ref().unwrap();
    assert_eq!(link.action, Action::Slide { slide: doc.slides[2].id });
    assert_eq!(link.zoom_return, None);
    assert_eq!(link.tooltip, "Plan");
    let ShapeKind::Picture { fill } = &zoom.kind else { panic!() };
    assert!(doc.media(fill.media).is_some_and(|m| m.data.starts_with(b"\x89PNG")));

    // Section zoom: back after the section's last slide.
    s.execute("section.add", &json!({"name": "Start", "at": 0})).unwrap();
    s.execute("section.add", &json!({"name": "Money", "at": 3})).unwrap();
    let doc = s.doc().unwrap().doc.clone();
    let money = doc.sections.iter().position(|x| x.name == "Money").unwrap();
    s.execute("slide.go", &json!({"index": 0})).unwrap();
    s.execute("insert.zoom", &json!({"kind": "section", "sections": [money]})).unwrap();
    let doc = s.doc().unwrap().doc.clone();
    let sec = doc.slides[0].shapes.iter().find(|sh| sh.name.starts_with("Section Zoom")).unwrap();
    let last = *doc.sections[money].slides.last().unwrap();
    assert_eq!(sec.click.as_ref().unwrap().zoom_return, Some(last));

    // Summary zoom on slides 1 and 3: a new slide in front, parts 1–2 and 3–4.
    let before = doc.slides.len();
    s.execute("insert.zoom", &json!({"kind": "summary", "slides": [1, 3]})).unwrap();
    let doc = s.doc().unwrap().doc.clone();
    assert_eq!(doc.slides.len(), before + 1);
    let sum = &doc.slides[1];
    assert!(sum.title().contains("Summary"));
    let tiles: Vec<_> = sum.shapes.iter().filter_map(|sh| sh.click.as_ref()).collect();
    assert_eq!(tiles.len(), 2);
    assert_eq!(tiles[0].zoom_return, Some(doc.slides[3].id), "part one runs to the slide before part two");
    assert_eq!(tiles[1].zoom_return, Some(doc.slides.last().unwrap().id));
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.doc().unwrap().doc.slides.len(), before, "one undo step");

    // Saved as linked pictures.
    let back = deckcraft_pptx::import(&deckcraft_pptx::export(&s.doc().unwrap().doc).unwrap()).unwrap();
    let z = back.slides[0].shapes.iter().find(|sh| matches!(sh.kind, ShapeKind::Picture { .. }) && sh.click.is_some()).unwrap();
    assert!(matches!(z.click.as_ref().unwrap().action, Action::Slide { .. }));
    assert!(s.execute("insert.zoom", &json!({"slides": [42]})).is_err());
    assert!(s.execute("insert.zoom", &json!({"kind": "slide"})).is_err());
}

#[test]
fn darken_and_desaturate_write_powerpoints_hsl_shift() {
    for (effect, preset, shift) in [("darken", 24, r#"s="0" l="-25098""#), ("desaturate", 25, r#"s="-70588" l="0""#)] {
        let mut p = defaults::new_presentation(None);
        let mut s = p.slides[0].as_ref().clone();
        let sh = shape(&mut p, "Target", 100.0, 100.0, 100.0, 100.0);
        let id = sh.id;
        s.shapes.push(sh);
        s.animations.push(Animation { shape: id, class: AnimClass::Emphasis, effect: effect.into(), duration_ms: 500, ..Default::default() });
        p.slides[0] = Arc::new(s);
        let bytes = deckcraft_pptx::export(&p).expect("export");
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut xml = String::new();
        std::io::Read::read_to_string(&mut zip.by_name("ppt/slides/slide1.xml").unwrap(), &mut xml).unwrap();
        assert!(xml.contains(&format!(r#"presetID="{preset}""#)), "{effect}");
        assert!(xml.contains(r#"clrSpc="hsl""#) && xml.contains(shift), "{effect}: {xml}");
        assert_eq!(round(&p).slides[0].animations[0].effect, effect);
    }
}

/// PowerPoint's default durations (COM `AddEffect`), which differ from the 0.5 s most effects get.
#[test]
fn default_durations_match_powerpoint() {
    for (effect, ms) in [
        ("grow", 1000),
        ("shrinkTurn", 1000),
        ("box", 2000),
        ("circle", 2000),
        ("diamond", 2000),
        ("boxOut", 2000),
        ("diamondOut", 2000),
        ("expand", 1000),
        ("contract", 1000),
        ("spinner", 500),
        ("spinnerOut", 500),
        ("colorPulse", 250),
        ("objectColor", 500),
        ("boldFlash", 2000),
    ] {
        let a = deckcraft_model::anim::ANIMATIONS.iter().find(|a| a.0 == effect).unwrap();
        assert_eq!(a.4, ms, "{effect}");
    }
}

/// Show media controls is PowerPoint's `p14:showMediaCtrls` extension inside `p:showPr`: off is
/// written there, under its extension uri and with the p14 namespace declared on the element;
/// on (the default) writes nothing.
#[test]
fn hidden_media_controls_survive_save_and_reopen() {
    let props = |p: &Presentation| {
        let bytes = deckcraft_pptx::export(p).expect("export");
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("zip");
        let mut x = String::new();
        std::io::Read::read_to_string(&mut zip.by_name("ppt/presProps.xml").expect("presProps"), &mut x).expect("read");
        x
    };
    let mut p = Presentation::default();
    p.show.show_media_controls = false;
    let xml = props(&p);
    let show = xml.split("<p:showPr").nth(1).and_then(|r| r.split("</p:showPr>").next()).expect("p:showPr");
    assert!(
        show.contains(
            "<p:ext uri=\"{2FDB2607-1784-4EEB-B798-7EB5836EED8A}\"><p14:showMediaCtrls xmlns:p14=\"http://schemas.microsoft.com/office/powerpoint/2010/main\" val=\"0\"/></p:ext>"
        ),
        "{xml}"
    );
    assert!(!round(&p).show.show_media_controls);
    let default = Presentation::default();
    assert!(!props(&default).contains("showMediaCtrls"));
    assert!(round(&default).show.show_media_controls);
}

#[test]
fn reflection_angles_and_skews_out_of_range_are_written_inside_the_schema() {
    let mut p = Presentation::default();
    let mut slide = new_slide(&mut p, "Reflection ranges");
    let mut s = shape(&mut p, "Reflected", 10.0, 10.0, 200.0, 80.0);
    s.effects = Some(deckcraft_model::style::Effects {
        reflection: Some(deckcraft_model::style::Reflection {
            st_pos: Some(150_000),
            dir: Some(-5),
            fade_dir: Some(21_600_000),
            kx: Some(9_000_000),
            ky: Some(-9_000_000),
            ..Default::default()
        }),
        ..Default::default()
    });
    slide.shapes.push(s);
    p.slides.push(Arc::new(slide));

    let r = round(&p).slides.last().and_then(|sl| by_name(sl, "Reflected").effects.clone()).and_then(|fx| fx.reflection).expect("reflection");
    assert_eq!((r.st_pos, r.dir, r.fade_dir, r.kx, r.ky), (Some(100_000), Some(21_599_995), Some(0), Some(5_399_999), Some(-5_399_999)));
}

#[test]
fn pptx_potx_ppsx_export_parity_preserves_masters_charts_themes_and_timing() {
    let mut p = rich_deck();
    let m0 = Arc::make_mut(&mut p.masters[0]);
    m0.theme.name = "ParityTheme".into();
    m0.theme.colors.set(deckcraft_color::SchemeSlot::Accent1, deckcraft_color::Rgba::rgb(0x11, 0x22, 0x33));
    p.notes_master = Some(Arc::new(deckcraft_model::Master { name: "NotesParity".into(), ..Default::default() }));
    p.handout_master = Some(Arc::new(deckcraft_model::Master { name: "HandoutParity".into(), ..Default::default() }));

    let audio = p.add_media("cue.m4a", "audio/mp4", b"audio-bytes".to_vec()).expect("media");
    let aud_id = ShapeId(p.alloc_id().expect("id"));
    Arc::make_mut(&mut p.slides[0]).shapes.push(Shape {
        id: aud_id,
        name: "Cue".into(),
        xfrm: Some(Xfrm::new(20.0, 20.0, 40.0, 40.0)),
        kind: ShapeKind::Media(deckcraft_model::MediaClip {
            media: audio,
            video: false,
            autoplay: true,
            loop_play: true,
            volume: 0.5,
            ..Default::default()
        }),
        ..Default::default()
    });

    let edited_values = vec![Some(111.5), Some(222.25), Some(333.75), Some(444.0)];
    let chart_slide_idx = p.slides.iter().position(|sl| sl.shapes.iter().any(|s| s.name == "Chart 0")).expect("chart slide");
    {
        let ch_shape = Arc::make_mut(&mut p.slides[chart_slide_idx]).shapes.iter_mut().find(|s| s.name == "Chart 0").expect("Chart 0");
        ch_shape.name = "Revenue Chart".into();
        let ShapeKind::Chart(ch) = &mut ch_shape.kind else { panic!("expected chart") };
        ch.series[0].values = edited_values.clone();
    }

    for (ext, expected_ct) in [
        ("pptx", "presentationml.presentation.main+xml"),
        ("potx", "presentationml.template.main+xml"),
        ("POTX", "presentationml.template.main+xml"),
        (".ppsx", "presentationml.slideshow.main+xml"),
    ] {
        let bytes = deckcraft_pptx::export_as(&p, ext).expect("export_as");
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).expect("zip");
        let ct = std::io::read_to_string(z.by_name("[Content_Types].xml").expect("content types")).expect("read");
        assert!(ct.contains(expected_ct), "{ext}: {ct}");
        let reopened = deckcraft_pptx::import(&bytes).expect("reopen");
        assert_eq!(reopened.masters[0].theme.name, "ParityTheme");
        assert_eq!(reopened.masters[0].theme.colors.get(deckcraft_color::SchemeSlot::Accent1), deckcraft_color::Rgba::rgb(0x11, 0x22, 0x33));
        assert!(reopened.notes_master.is_some());
        assert_eq!(reopened.handout_master.as_ref().map(|m| m.name.as_str()), Some("HandoutParity"));
        let cue = reopened.slides[0].shapes.iter().find(|s| s.name == "Cue").expect("cue");
        let ShapeKind::Media(clip) = &cue.kind else { panic!("expected media") };
        assert!(clip.autoplay && clip.loop_play && (clip.volume - 0.5).abs() < 1e-3);
        let chart_shape = reopened.slides[chart_slide_idx].shapes.iter().find(|s| s.name == "Revenue Chart").expect("named chart");
        let ShapeKind::Chart(chart) = &chart_shape.kind else { panic!("expected chart") };
        assert_eq!(chart.series[0].values, edited_values, "{ext}");
    }
}
