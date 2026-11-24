use super::{
    ColorRef, Fill, LayoutType, Paragraph, PhType, Placeholder, Presentation, Run, RunProps, SchemeSlot, Shape, ShapeId, ShapeKind, ShapeStyle,
    Slide, Table, TextBody, Theme, Xfrm, defaults, ink_pressure_is_legacy, ink_sample_width, parent_of, resolve, sibling_list_mut, style, text,
    theme, walk,
};
use crate::resolve::Ctx;

#[test]
fn new_presentation_is_valid() {
    let p = Presentation::default();
    assert_eq!(p.slides.len(), 1);
    assert_eq!(p.masters.len(), 1);
    assert_eq!(p.masters[0].layouts.len(), 11);
    assert!(p.validate().is_empty(), "{:?}", p.validate());
    let s = &p.slides[0];
    let (m, l) = p.master_for(s).unwrap();
    assert_eq!(l.unwrap().kind, LayoutType::Title);
    assert_eq!(m.theme.name, "DeckCraft");
    // Title slide has a centred title and a subtitle.
    let kinds: Vec<_> = s.shapes.iter().filter_map(|x| x.ph_type()).collect();
    assert_eq!(kinds, vec![PhType::CtrTitle, PhType::SubTitle]);
}

#[test]
fn placeholder_inherits_position_and_styles() {
    let p = Presentation::default();
    let s = &p.slides[0];
    let ctx = Ctx::for_slide(&p, s).unwrap();
    let title = &s.shapes[0];
    let x = resolve::xfrm(&ctx, title);
    assert!((x.x - 120.0).abs() < 1e-9 && (x.w - 720.0).abs() < 1e-9);
    let lvl = resolve::level_style(&ctx, title, 0);
    assert_eq!(lvl.run.size, Some(60.0)); // layout overrides the master's 44
    assert_eq!(lvl.para.align, Some(text::Align::Center));
    assert_eq!(resolve::font_family(&ctx, &lvl.run), "Inter");
    let body = resolve::body(&ctx, title);
    assert_eq!(body.anchor, Some(text::Anchor::Bottom));
}

#[test]
fn body_levels_from_master() {
    let mut p = Presentation::default();
    let lay = defaults::layout_of_kind(&p, LayoutType::TitleAndContent).unwrap();
    let s = defaults::new_slide(&mut p, lay).expect("slide");
    let ctx = Ctx::for_slide(&p, &s).unwrap();
    let body = &s.shapes[1];
    assert_eq!(body.ph_type(), Some(PhType::Obj));
    let l0 = resolve::level_style(&ctx, body, 0);
    let l1 = resolve::level_style(&ctx, body, 1);
    assert_eq!(l0.run.size, Some(28.0));
    assert_eq!(l1.run.size, Some(24.0));
    assert!(matches!(l0.para.bullet, Some(text::Bullet::Char { .. })));
}

#[test]
fn shape_style_fill_resolves_through_theme() {
    let p = Presentation::default();
    let s = &p.slides[0];
    let ctx = Ctx::for_slide(&p, s).unwrap();
    let sh = Shape { style: Some(ShapeStyle::accent(SchemeSlot::Accent1)), xfrm: Some(Xfrm::new(0.0, 0.0, 10.0, 10.0)), ..Default::default() };
    let (f, ph) = resolve::fill(&ctx, &sh);
    let Some(Fill::Solid { color }) = f else { panic!("expected solid") };
    assert_eq!(ctx.color(&color, ph), p.masters[0].theme.colors.get(SchemeSlot::Accent1));
    let (l, ph) = resolve::line(&ctx, &sh);
    assert!(l.width.is_some());
    assert!(ph.is_some());
}

#[test]
fn a_gradient_without_stops_takes_them_from_its_style() {
    // `a:gradFill` with only a direction: PowerPoint fills it with the stops of the theme gradient
    // the style's fillRef names, in the style's colour, along the shape's own direction.
    let p = Presentation::default();
    let s = &p.slides[0];
    let ctx = Ctx::for_slide(&p, s).unwrap();
    let own = style::Gradient {
        tile: [0.0; 4],
        stops: vec![],
        shape: style::GradientShape::Linear { angle: 45.0, scaled: true },
        rotate_with_shape: true,
        flip: None,
    };
    let mut st = ShapeStyle::accent(SchemeSlot::Accent2);
    st.fill_ref.0 = 3;
    let sh = Shape { style: Some(st.clone()), fill: Some(Fill::Gradient(own.clone())), ..Default::default() };
    let (f, ph) = resolve::fill(&ctx, &sh);
    let Some(Fill::Gradient(g)) = f else { panic!("expected a gradient") };
    let Some(Fill::Gradient(theme)) = p.masters[0].theme.format.fills.get(2).cloned() else { panic!("theme fill 3 is a gradient") };
    assert_eq!(g.stops, theme.stops);
    assert_eq!(g.shape, own.shape, "the shape's own direction");
    assert_eq!(ph, Some(p.masters[0].theme.colors.get(SchemeSlot::Accent2)));
    // A solid style fill has no stops to give: the gradient stays empty (draws nothing).
    st.fill_ref.0 = 1;
    let sh = Shape { style: Some(st), fill: Some(Fill::Gradient(own.clone())), ..Default::default() };
    assert_eq!(resolve::fill(&ctx, &sh).0, Some(Fill::Gradient(own)));
}

#[test]
fn serde_roundtrip() {
    let p = Presentation::default();
    let j = serde_json::to_string(&p).unwrap();
    let q: Presentation = serde_json::from_str(&j).unwrap();
    assert_eq!(p, q);
    // Unknown and missing fields are tolerated.
    let q: Presentation = serde_json::from_str(r#"{"slides":[],"zzz":1}"#).unwrap();
    assert!(q.slides.is_empty());
}

/// A deck whose ids are used up (a hostile file can arrive that way) yields no further ids:
/// allocating never repeats one (#407).
#[test]
fn exhausted_id_space_allocates_nothing() {
    let mut p = Presentation { next_id: u32::MAX, ..Default::default() };
    assert_eq!(p.alloc_id(), None);
    assert_eq!(p.alloc_id(), None);
    assert!(p.add_media("x.png", "image/png", vec![9]).is_none(), "fresh media bytes are refused too");
    let lay = defaults::layout_of_kind(&p, LayoutType::TitleAndContent).unwrap_or_default();
    assert!(defaults::new_slide(&mut p, lay).is_none(), "a new slide is refused");
}

#[test]
fn ids_are_unique_and_fix_next_id() {
    let mut p = Presentation::default();
    let a = p.alloc_id().expect("ids");
    let b = p.alloc_id().expect("ids");
    assert!(b > a);
    p.next_id = 0;
    p.fix_next_id();
    assert!(p.next_id >= b.min(p.next_id));
    let c = p.alloc_id().expect("ids");
    let mut all = vec![];
    for s in &p.slides {
        all.push(s.id.0);
        walk(&s.shapes, &mut |sh, _| all.push(sh.id.0));
    }
    assert!(!all.contains(&c));
}

#[test]
fn group_find_and_walk() {
    let child = Shape { id: ShapeId(900), ..Default::default() };
    let g = Shape { id: ShapeId(901), kind: ShapeKind::Group { children: vec![child], child: Xfrm::new(0.0, 0.0, 1.0, 1.0) }, ..Default::default() };
    let mut slide = Slide { shapes: vec![g], ..Default::default() };
    assert!(slide.shape(ShapeId(900)).is_some());
    assert!(slide.shape_mut(ShapeId(900)).is_some());
    let mut n = 0;
    walk(&slide.shapes, &mut |_, _| n += 1);
    assert_eq!(n, 2);
}

#[test]
fn text_body_helpers() {
    let mut t = TextBody::from_text("Hello\nWorld");
    assert_eq!(t.text(), "Hello\nWorld");
    assert_eq!(t.char_len(), 11);
    t.paragraphs[0].runs.push(Run::new(" there"));
    t.paragraphs[0].normalize();
    assert_eq!(t.paragraphs[0].runs.len(), 1);
    assert_eq!(t.paragraphs[0].text(), "Hello there");
}

#[test]
fn builtin_themes_have_distinct_names() {
    let t = theme::builtin_themes();
    let mut names: Vec<_> = t.iter().map(|t| t.name.clone()).collect();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), t.len());
    assert!(t.len() >= 8);
}

#[test]
fn four_by_three_master_scales() {
    let p = defaults::blank_presentation(defaults::STANDARD, Theme::default(), true);
    let m = &p.masters[0];
    let title = m.shapes.iter().find(|s| s.ph_type() == Some(PhType::Title)).unwrap();
    let x = title.xfrm.unwrap();
    assert!(x.x + x.w <= 720.0 + 1e-6);
}

#[test]
fn hyperlinked_runs_take_the_theme_hyperlink_look() {
    use crate::text::{Action, Hyperlink, Paragraph, RunProps};
    let p = Presentation::default();
    let s = &p.slides[0];
    let ctx = Ctx::for_slide(&p, s).unwrap();
    let title = &s.shapes[0];
    let para = Paragraph::default();
    let link = Some(Hyperlink {
        action: Action::Url { url: "https://example.org".into() },
        tooltip: String::new(),
        highlight_click: false,
        sound: None,
        stop_sound: false,
        zoom_return: None,
    });
    let hlink = Some(Fill::solid(ColorRef::scheme(SchemeSlot::Hlink)));
    // The title's master style colours its text tx1; a link is hlink and underlined instead.
    assert_eq!(resolve::run(&ctx, title, &para, &RunProps::default()).fill, Some(Fill::solid(ColorRef::scheme(SchemeSlot::Tx1))));
    let r = resolve::run(&ctx, title, &para, &RunProps { link: link.clone(), ..Default::default() });
    assert_eq!((r.fill, r.underline.as_deref()), (hlink.clone(), Some("sng")));
    assert_eq!(ctx.color(&ColorRef::scheme(SchemeSlot::Hlink), None), p.masters[0].theme.colors.get(SchemeSlot::Hlink));
    // A slide link looks the same.
    let slide = Some(Hyperlink {
        action: Action::Slide { slide: s.id },
        tooltip: String::new(),
        highlight_click: false,
        sound: None,
        stop_sound: false,
        zoom_return: None,
    });
    assert_eq!(resolve::run(&ctx, title, &para, &RunProps { link: slide, ..Default::default() }).fill, hlink);
    // The run's own colour and underline win; u="none" removes the underline.
    let red = Some(Fill::solid(ColorRef::rgb(deckcraft_color::Rgba::rgb(255, 0, 0))));
    let own = RunProps { link: link.clone(), fill: red.clone(), underline: Some("none".into()), ..Default::default() };
    let r = resolve::run(&ctx, title, &para, &own);
    assert_eq!((r.fill, r.underline.as_deref()), (red, Some("none")));
    let dbl = resolve::run(&ctx, title, &para, &RunProps { link, underline: Some("dbl".into()), ..Default::default() });
    assert_eq!((dbl.fill, dbl.underline.as_deref()), (hlink, Some("dbl")));
}

#[test]
fn a_layout_colour_map_override_applies_to_its_slides() {
    // A dark "section" layout maps bg1→dk1 and tx1→lt1; a slide on it that keeps the master
    // mapping (`masterClrMapping`) follows the layout, as PowerPoint draws it.
    let mut p = Presentation::default();
    let dark: Vec<(String, String)> =
        [("bg1", "dk1"), ("tx1", "lt1"), ("bg2", "dk2"), ("tx2", "lt2")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
    let layout = p.slides[0].layout;
    let m = std::sync::Arc::make_mut(&mut p.masters[0]);
    let colors = m.theme.colors.clone();
    m.layouts.iter_mut().find(|l| l.id == layout).unwrap().color_map = Some(dark.clone());
    let s: Slide = (*p.slides[0]).clone();
    let ctx = Ctx::for_slide(&p, &s).unwrap();
    assert_eq!(ctx.color(&ColorRef::scheme(SchemeSlot::Bg1), None), colors.get(SchemeSlot::Dk1));
    assert_eq!(ctx.color(&ColorRef::scheme(SchemeSlot::Tx2), None), colors.get(SchemeSlot::Lt2));
    assert_eq!(ctx.scheme.get(SchemeSlot::Lt1), colors.get(SchemeSlot::Dk1));
    // The slide's own override wins over the layout's.
    let mut own = s.clone();
    own.color_map = Some([("bg1", "lt1"), ("tx1", "dk1")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect());
    let ctx = Ctx::for_slide(&p, &own).unwrap();
    assert_eq!(ctx.color(&ColorRef::scheme(SchemeSlot::Bg1), None), colors.get(SchemeSlot::Lt1));
    // The layout itself draws with its override; the master keeps its own map.
    let m = &p.masters[0];
    let l = m.layout(layout).unwrap();
    assert_eq!(Ctx::for_layout(&p, m, l).color(&ColorRef::scheme(SchemeSlot::Bg1), None), colors.get(SchemeSlot::Dk1));
    assert_eq!(Ctx::for_master(&p, m).color(&ColorRef::scheme(SchemeSlot::Bg1), None), colors.get(SchemeSlot::Lt1));
}

#[test]
fn sibling_lists_and_parents_reach_into_groups() {
    let leaf = |id: u32| Shape { id: ShapeId(id), ..Default::default() };
    let group = |id: u32, children: Vec<Shape>| Shape {
        id: ShapeId(id),
        kind: ShapeKind::Group { children, child: Default::default() },
        ..Default::default()
    };
    let mut tree = vec![leaf(1), group(2, vec![leaf(3), group(4, vec![leaf(5)])]), leaf(6)];
    assert_eq!(parent_of(&tree, ShapeId(1)), Some(None));
    assert_eq!(parent_of(&tree, ShapeId(3)), Some(Some(ShapeId(2))));
    assert_eq!(parent_of(&tree, ShapeId(5)), Some(Some(ShapeId(4))));
    assert_eq!(parent_of(&tree, ShapeId(9)), None);
    let ids = |v: &Vec<Shape>| v.iter().map(|s| s.id.0).collect::<Vec<_>>();
    assert_eq!(sibling_list_mut(&mut tree, ShapeId(6)).map(|v| ids(v)), Some(vec![1, 2, 6]));
    assert_eq!(sibling_list_mut(&mut tree, ShapeId(4)).map(|v| ids(v)), Some(vec![3, 4]));
    assert_eq!(sibling_list_mut(&mut tree, ShapeId(5)).map(|v| ids(v)), Some(vec![5]));
    assert!(sibling_list_mut(&mut tree, ShapeId(9)).is_none());
}

#[test]
fn ink_width_follows_pressure_and_keeps_the_old_placeholder() {
    assert!(ink_pressure_is_legacy(&[(0.0, 0.0, 0.5), (1.0, 1.0, 0.5)]));
    assert!(!ink_pressure_is_legacy(&[(0.0, 0.0, 0.5), (1.0, 1.0, 1.0)]));
    assert!(!ink_pressure_is_legacy(&[]));
    assert!((ink_sample_width(8.0, 0.5, true) - 8.0).abs() < 1e-9);
    assert!((ink_sample_width(8.0, 1.0, false) - 8.0).abs() < 1e-9);
    assert!((ink_sample_width(8.0, 0.25, false) - 2.0).abs() < 1e-9);
    let floor = ink_sample_width(8.0, 0.0, false);
    assert!((floor - 0.64).abs() < 1e-4, "a light touch stays visible, got {floor}");
    assert!((ink_sample_width(8.0, f32::NAN, false) - 8.0).abs() < 1e-9);
}

#[test]
fn table_text_takes_the_masters_other_style_and_shapes_the_default_text_style() {
    // PowerPoint for Mac, with an otherStyle of 14 pt, a defaultTextStyle of 26 pt and a 36 pt
    // bodyStyle: a table cell's text is 14 pt, also in a table placed in a content placeholder; a
    // shape's and a text box's is 26 pt.
    let mut p = Presentation::default();
    let size = |pt: f64| crate::text::LevelStyle { run: RunProps { size: Some(pt), ..Default::default() }, ..Default::default() };
    p.default_text_style.set(0, size(26.0));
    std::sync::Arc::make_mut(&mut p.masters[0]).other_style.set(0, size(14.0));
    std::sync::Arc::make_mut(&mut p.masters[0]).body_style.set(0, size(36.0));
    let s = p.slides[0].clone();
    let ctx = Ctx::for_slide(&p, &s).unwrap();
    let para = Paragraph::default();
    let pt = |sh: &Shape| resolve::run(&ctx, sh, &para, &RunProps::default()).size;
    let body = crate::text::TextBody::from_text("Cell");
    assert_eq!(pt(&Shape::table_cell(body.clone())), Some(14.0), "a table cell");
    let table = Shape { kind: ShapeKind::Table(Table::default()), ..Default::default() };
    assert_eq!(pt(&table), Some(14.0), "the table itself (its formatting state)");
    let in_placeholder = Shape { ph: Some(Placeholder { kind: PhType::Obj, idx: 1, ..Default::default() }), ..table };
    assert_eq!(pt(&in_placeholder), Some(14.0), "a table in a content placeholder");
    assert_eq!(pt(&Shape { text: Some(body.clone()), ..Default::default() }), Some(26.0), "a shape");
    assert_eq!(pt(&Shape { text: Some(body), text_box: true, ..Default::default() }), Some(26.0), "a text box");
}

#[test]
fn only_web_and_mail_links_are_safe_to_open() {
    use crate::text::safe_url;
    for url in ["https://example.com", " HTTP://example.com/x", "mailto:someone@example.com"] {
        assert!(safe_url(url), "{url}");
    }
    let refused = [
        "https://",
        "mailto:",
        "file:///C:/notes.pdf",
        "notes.pdf",
        "javascript:alert(1)",
        "ms-msdt:/id PCWDiagnostic",
        "ldap://attacker.example/x",
        "//attacker.example/share",
        "\\\\attacker.example\\share",
        "smb://attacker.example/share",
        "search-ms:query=x",
    ];
    for url in refused {
        assert!(!safe_url(url), "{url}");
    }
}

#[test]
fn colour_alpha_follows_alpha_mod_and_off() {
    use deckcraft_color::{ColorTransform, Rgba};
    let base = || ColorRef::rgb(Rgba::rgb(1, 2, 3));
    let mod_half = base().with(ColorTransform::AlphaMod(50000));
    assert!((mod_half.alpha() - 0.5).abs() < 1e-9, "{}", mod_half.alpha());
    let off = base().with(ColorTransform::AlphaOff(-50000));
    assert!((off.alpha() - 0.5).abs() < 1e-9, "{}", off.alpha());
    let both = base().with(ColorTransform::Alpha(40000)).with(ColorTransform::AlphaMod(50000));
    assert!((both.alpha() - 0.2).abs() < 1e-9, "{}", both.alpha());
}
