//! Owned round trip: comments, hyperlinks and actions, custom shows, grouped
//! rotated objects, and placeholders that inherit from the layout.
//!
//! Embedded fonts stay out of this check; another open change owns that part.

use std::sync::Arc;

use deckcraft_color::Rgba;
use deckcraft_geom::Xfrm;
use deckcraft_model::style::{ColorRef, Fill};
use deckcraft_model::text::{Action, Hyperlink, LevelStyle, ListStyle, Paragraph, Run, RunProps, TextBody};
use deckcraft_model::{Comment, CustomShow, Geom, LayoutType, PhType, Presentation, Shape, ShapeId, ShapeKind, defaults, resolve};

const CHIME: &[u8] = b"RIFFdeckcraft-chime-WAVEfmt ";

fn link(action: Action) -> Hyperlink {
    Hyperlink { action, tooltip: String::new(), highlight_click: false, sound: None, stop_sound: false, zoom_return: None }
}

fn box_at(p: &mut Presentation, name: &str, x: f64, y: f64) -> Shape {
    Shape { id: ShapeId(p.alloc_id().expect("id")), name: name.into(), xfrm: Some(Xfrm::new(x, y, 80.0, 28.0)), ..Default::default() }
}

fn round(p: &Presentation) -> Presentation {
    deckcraft_pptx::import(&deckcraft_pptx::export(p).expect("export")).expect("import")
}

fn shape<'a>(p: &'a Presentation, slide: usize, name: &str) -> &'a Shape {
    p.slides[slide].shapes.iter().find(|s| s.name == name).unwrap_or_else(|| panic!("missing {name}"))
}

fn placeholder(p: &Presentation, kind: PhType) -> &Shape {
    p.slides[0].shapes.iter().find(|s| s.ph_type() == Some(kind)).unwrap_or_else(|| panic!("missing {kind:?}"))
}

fn deck() -> Presentation {
    let mut p = defaults::new_presentation(None);
    let layout = defaults::layout_of_kind(&p, LayoutType::TitleAndContent).expect("layout");
    let master = Arc::make_mut(&mut p.masters[0]);
    let lay = master.layouts.iter_mut().find(|l| l.id == layout).expect("layout");
    for s in &mut lay.shapes {
        if s.ph_type() != Some(PhType::Obj) {
            continue;
        }
        s.xfrm = Some(Xfrm::new(48.0, 120.0, 600.0, 300.0));
        s.geom = Geom::preset("ellipse");
        s.fill = Some(Fill::solid(ColorRef::rgb(Rgba::rgb(20, 40, 80))));
        let mut ls = ListStyle::default();
        ls.set(0, LevelStyle { run: RunProps { size: Some(28.0), font: Some("Source Serif 4".into()), ..Default::default() }, ..Default::default() });
        let body = s.text.get_or_insert_with(TextBody::default);
        body.list_style = ls;
    }

    let chime = p.add_media("chime.wav", "audio/wav", CHIME.to_vec()).expect("media");
    let mut a = defaults::new_slide(&mut p, layout).expect("slide");
    a.name = "Review".into();
    if let Some(title) = a.shapes.iter_mut().find(|s| s.ph_type() == Some(PhType::Title)) {
        title.text = Some(TextBody::from_text("Review"));
    }
    if let Some(body) = a.shapes.iter_mut().find(|s| s.ph_type() == Some(PhType::Obj)) {
        body.text = Some(TextBody {
            paragraphs: vec![
                Paragraph {
                    runs: vec![Run::new("Quoted")],
                    def_props: RunProps {
                        size: Some(22.0),
                        bold: Some(true),
                        font: Some("Literata".into()),
                        link: Some(Hyperlink {
                            action: Action::Url { url: "https://example.com/quote".into() },
                            tooltip: "Source".into(),
                            highlight_click: false,
                            sound: None,
                            stop_sound: false,
                            zoom_return: None,
                        }),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                Paragraph { runs: vec![Run::new("Inherits")], ..Default::default() },
            ],
            ..Default::default()
        });
    }
    let mut gem = box_at(&mut p, "Gem", 12.0, 18.0);
    gem.xfrm = Some(Xfrm { x: 12.0, y: 18.0, w: 80.0, h: 40.0, rot: 20.0, flip_v: true, ..Default::default() });
    gem.text = Some(TextBody::from_text("Tilted"));
    let mut cluster = box_at(&mut p, "Cluster", 80.0, 40.0);
    cluster.xfrm = Some(Xfrm { x: 80.0, y: 40.0, w: 220.0, h: 140.0, rot: 25.0, flip_h: true, ..Default::default() });
    cluster.kind = ShapeKind::Group { children: vec![gem], child: Xfrm::new(10.0, 20.0, 400.0, 240.0) };
    a.shapes.push(cluster);

    let mut site = box_at(&mut p, "Site", 40.0, 460.0);
    site.click = Some(Hyperlink {
        action: Action::Url { url: "https://example.com/deck".into() },
        tooltip: "Example".into(),
        highlight_click: true,
        sound: None,
        stop_sound: false,
        zoom_return: None,
    });
    a.shapes.push(site);

    a.shapes.push(box_at(&mut p, "Go second", 140.0, 460.0));

    for (name, x, action) in [
        ("Next", 240.0, Action::NextSlide),
        ("End", 330.0, Action::EndShow),
        ("Back", 420.0, Action::LastViewed),
        ("Last", 510.0, Action::LastSlide),
        ("Open file", 600.0, Action::Program { path: "Talk notes.txt".into() }),
        ("Play clip", 700.0, Action::PlayMedia),
    ] {
        let mut s = box_at(&mut p, name, x, 500.0);
        s.click = Some(link(action));
        a.shapes.push(s);
    }

    let mut hover = box_at(&mut p, "Hover", 40.0, 500.0);
    hover.hover = Some(Hyperlink {
        action: Action::PreviousSlide,
        tooltip: "Back one".into(),
        highlight_click: false,
        sound: None,
        stop_sound: true,
        zoom_return: None,
    });
    a.shapes.push(hover);

    let mut home = box_at(&mut p, "Home", 800.0, 460.0);
    home.geom = Geom::preset("actionButtonHome");
    home.click = Some(Hyperlink {
        action: Action::FirstSlide,
        tooltip: String::new(),
        highlight_click: true,
        sound: None,
        stop_sound: false,
        zoom_return: None,
    });
    a.shapes.push(home);

    let mut board = box_at(&mut p, "To board", 140.0, 500.0);
    board.click = Some(Hyperlink {
        action: Action::CustomShow { name: "Board".into(), returns: false },
        tooltip: "Open the board show".into(),
        highlight_click: false,
        sound: Some(chime),
        stop_sound: false,
        zoom_return: None,
    });
    a.shapes.push(board);

    let mut short = box_at(&mut p, "To short", 240.0, 500.0);
    short.click = Some(link(Action::CustomShow { name: "Short".into(), returns: true }));
    a.shapes.push(short);

    a.comments = vec![Comment {
        author: "Mina".into(),
        initials: "ML".into(),
        text: "Check the quote".into(),
        date: "2026-03-01T09:30:00.000".into(),
        x: 36.0,
        y: 48.0,
        ..Default::default()
    }];

    let mut b = defaults::new_slide(&mut p, layout).expect("slide");
    if let Some(title) = b.shapes.iter_mut().find(|s| s.ph_type() == Some(PhType::Title)) {
        title.text = Some(TextBody::from_text("Second"));
    }
    // The jump is filled in once this slide has an id.
    let b_id = b.id;
    if let Some(go) = a.shapes.iter_mut().find(|s| s.name == "Go second") {
        go.click = Some(link(Action::Slide { slide: b_id }));
    }
    p.slides = vec![Arc::new(a), Arc::new(b)];
    let ids: Vec<_> = p.slides.iter().map(|s| s.id).collect();
    p.custom_shows =
        vec![CustomShow { name: "Short".into(), slides: vec![ids[0]] }, CustomShow { name: "Board".into(), slides: vec![ids[1], ids[0]] }];
    p.show.custom_show = Some("Board".into());
    p
}

fn assert_deck(q: &Presentation) {
    assert_eq!(q.slides.len(), 2);
    assert_eq!(placeholder(q, PhType::Title).text.as_ref().map(|t| t.text()), Some("Review".into()));

    let body = placeholder(q, PhType::Obj);
    assert!(body.xfrm.is_none(), "a placeholder with no box of its own keeps inheriting the layout");
    assert!(matches!(&body.geom, Geom::Preset { name, .. } if name == "ellipse"), "layout geometry: {:?}", body.geom);
    let ctx = resolve::Ctx::for_slide(q, &q.slides[0]).expect("ctx");
    let x = resolve::xfrm(&ctx, body);
    assert_eq!((x.x, x.y, x.w, x.h), (48.0, 120.0, 600.0, 300.0));
    let (fill, _) = resolve::fill(&ctx, body);
    assert_eq!(fill, Some(Fill::solid(ColorRef::rgb(Rgba::rgb(20, 40, 80)))));

    let paras = &body.text.as_ref().expect("text").paragraphs;
    assert_eq!(paras[0].runs[0].text, "Quoted");
    assert!(paras[0].runs[0].props.size.is_none(), "the run itself stays unset");
    assert_eq!(paras[0].def_props.size, Some(22.0));
    assert_eq!(paras[0].def_props.bold, Some(true));
    assert_eq!(paras[0].def_props.font.as_deref(), Some("Literata"));
    match &paras[0].def_props.link {
        Some(h) => match &h.action {
            Action::Url { url } => assert_eq!(url, "https://example.com/quote"),
            other => panic!("paragraph link: {other:?}"),
        },
        None => panic!("paragraph default run lost its link"),
    }
    assert_eq!(paras[0].def_props.link.as_ref().map(|h| h.tooltip.as_str()), Some("Source"));
    let quoted = resolve::run(&ctx, body, &paras[0], &paras[0].runs[0].props);
    assert_eq!(quoted.size, Some(22.0));
    assert_eq!(paras[1].runs[0].text, "Inherits");
    let inherited = resolve::run(&ctx, body, &paras[1], &paras[1].runs[0].props);
    assert_eq!(inherited.size, Some(28.0), "the second paragraph takes the layout list style");
    assert_eq!(inherited.font.as_deref(), Some("Source Serif 4"));
    assert_group(q);
    assert_comments(q);
    assert_click_links(q);
    assert_custom_show_links(q);
    assert_custom_shows(q);
}

fn assert_group(q: &Presentation) {
    let cluster = shape(q, 0, "Cluster");
    let g = cluster.xfrm.expect("group box");
    assert_eq!((g.x, g.y, g.w, g.h, g.rot, g.flip_h), (80.0, 40.0, 220.0, 140.0, 25.0, true));
    let ShapeKind::Group { children, child } = &cluster.kind else { panic!("not a group") };
    assert_eq!((child.x, child.y, child.w, child.h), (10.0, 20.0, 400.0, 240.0));
    let gem = &children[0];
    assert_eq!(gem.text.as_ref().map(|t| t.text()), Some("Tilted".into()));
    let gx = gem.xfrm.expect("gem");
    assert_eq!((gx.x, gx.y, gx.w, gx.h, gx.rot, gx.flip_v), (12.0, 18.0, 80.0, 40.0, 20.0, true));
}

fn assert_comments(q: &Presentation) {
    let c = &q.slides[0].comments;
    assert_eq!(c.len(), 1);
    assert_eq!((c[0].author.as_str(), c[0].initials.as_str(), c[0].text.as_str()), ("Mina", "ML", "Check the quote"));
    assert_eq!(c[0].date, "2026-03-01T09:30:00.000");
    assert_eq!((c[0].x, c[0].y), (36.0, 48.0));
}

fn assert_click_links(q: &Presentation) {
    let click = |name: &str| shape(q, 0, name).click.clone().unwrap_or_else(|| panic!("{name} lost its click"));
    match click("Site").action {
        Action::Url { url } => assert_eq!(url, "https://example.com/deck"),
        other => panic!("{other:?}"),
    }
    assert_eq!(click("Site").tooltip, "Example");
    assert!(click("Site").highlight_click);
    match click("Go second").action {
        Action::Slide { slide } => assert_eq!(q.slides.iter().position(|s| s.id == slide), Some(1)),
        other => panic!("{other:?}"),
    }
    assert!(matches!(click("Next").action, Action::NextSlide));
    assert!(matches!(click("End").action, Action::EndShow));
    assert!(matches!(click("Back").action, Action::LastViewed));
    assert!(matches!(click("Last").action, Action::LastSlide));
    match click("Open file").action {
        Action::Program { path } => assert_eq!(path, "Talk notes.txt"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(click("Play clip").action, Action::PlayMedia));
    assert!(matches!(click("Home").action, Action::FirstSlide));
    assert!(click("Home").highlight_click);
    assert!(matches!(shape(q, 0, "Home").geom, Geom::Preset { ref name, .. } if name == "actionButtonHome"));
    let hover = shape(q, 0, "Hover").hover.clone().expect("hover");
    assert!(matches!(hover.action, Action::PreviousSlide));
    assert!(hover.stop_sound);
    assert_eq!(hover.tooltip, "Back one");
}

fn assert_custom_show_links(q: &Presentation) {
    let click = |name: &str| shape(q, 0, name).click.clone().unwrap_or_else(|| panic!("{name} lost its click"));
    match click("To board").action {
        Action::CustomShow { name, returns } => {
            assert_eq!(name, "Board");
            assert!(!returns, "Show and return was forced on");
        }
        other => panic!("{other:?}"),
    }
    let snd = click("To board").sound.expect("action sound");
    assert_eq!(q.media(snd).map(|m| m.data.as_ref().clone()).as_deref(), Some(CHIME));
    match click("To short").action {
        Action::CustomShow { name, returns } => {
            assert_eq!(name, "Short");
            assert!(returns);
        }
        other => panic!("{other:?}"),
    }
}

fn assert_custom_shows(q: &Presentation) {
    assert_eq!(q.custom_shows.len(), 2);
    assert_eq!(q.custom_shows[0].name, "Short");
    assert_eq!(q.custom_shows[0].slides, vec![q.slides[0].id]);
    assert_eq!(q.custom_shows[1].name, "Board");
    assert_eq!(q.custom_shows[1].slides, vec![q.slides[1].id, q.slides[0].id]);
    assert_eq!(q.show.custom_show.as_deref(), Some("Board"));
    assert_eq!(
        q.slides[1].shapes.iter().find(|s| s.ph_type() == Some(PhType::Title)).and_then(|s| s.text.as_ref()).map(|t| t.text()),
        Some("Second".into())
    );
}

#[test]
fn comments_links_shows_groups_and_inherited_placeholders_round_trip() {
    let p = deck();
    let once = round(&p);
    assert_deck(&once);
    let twice = round(&once);
    assert_deck(&twice);
}
