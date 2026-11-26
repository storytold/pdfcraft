//! A new presentation: one master with the standard set of layouts, master text styles, and a
//! first title slide. Positions and sizes are our own (16:9 at 960 × 540 pt, 4:3 scaled).

use std::sync::Arc;

use deckcraft_color::SchemeSlot;
use deckcraft_geom::{Size, Xfrm};

use crate::style::ColorRef;
use crate::text::{Anchor, AutoFit, BodyProps, Bullet, LevelStyle, ListStyle, ParaProps, RunProps, Spacing, TextBody, TextDir};
use crate::theme::Theme;
use crate::{
    Background, Fill, HeaderFooter, Layout, LayoutId, LayoutType, Master, MasterId, PhType, Placeholder, Presentation, Shape, ShapeId, ShapeKind,
    Slide, SlideId,
};

pub const WIDE: Size = Size::new(960.0, 540.0);
pub const STANDARD: Size = Size::new(720.0, 540.0);

/// Slide size presets for Design ▸ Slide Size (label, width, height in points).
pub const SLIDE_SIZES: &[(&str, f64, f64)] = &[
    ("Widescreen (16:9)", 960.0, 540.0),
    ("Standard (4:3)", 720.0, 540.0),
    ("On-screen Show (16:10)", 720.0, 450.0),
    ("Letter Paper (8.5x11 in)", 720.0, 540.0),
    ("Ledger Paper (11x17 in)", 960.0, 720.0),
    ("A3 Paper (297x420 mm)", 1008.0, 756.0),
    ("A4 Paper (210x297 mm)", 780.0, 540.0),
    ("B4 (ISO) Paper (250x353 mm)", 852.0, 639.0),
    ("B5 (ISO) Paper (176x250 mm)", 598.0, 448.5),
    ("35mm Slides", 810.0, 540.0),
    ("Overhead", 720.0, 540.0),
    ("Banner", 576.0, 72.0),
    ("Widescreen (13.333x7.5 in)", 960.0, 540.0),
    ("Square (1:1)", 540.0, 540.0),
    ("Portrait (9:16)", 540.0, 960.0),
];

fn ph(kind: PhType, idx: u32) -> Placeholder {
    Placeholder { kind, idx, ..Default::default() }
}

fn body_with(anchor: Option<Anchor>, autofit: Option<AutoFit>) -> TextBody {
    TextBody { body: BodyProps { anchor, autofit, ..Default::default() }, ..Default::default() }
}

struct B {
    next: u32,
}

impl B {
    fn shape(&mut self, name: &str, kind: PhType, idx: u32, [x, y, w, h]: [f64; 4]) -> Shape {
        self.next += 1;
        Shape {
            id: ShapeId(self.next),
            name: name.into(),
            xfrm: Some(Xfrm::new(x, y, w, h)),
            ph: Some(ph(kind, idx)),
            text: Some(TextBody::default()),
            ..Default::default()
        }
    }
}

fn level(size: f64, margin: f64, indent: f64, before: f64) -> LevelStyle {
    LevelStyle {
        para: ParaProps {
            align: Some(crate::text::Align::Left),
            margin_left: Some(margin),
            indent: Some(indent),
            line_spacing: Some(Spacing::Pct(0.9)),
            space_before: Some(Spacing::Pts(before)),
            bullet: Some(Bullet::Char { char: "•".into() }),
            bullet_font: Some("Arial".into()),
            ..Default::default()
        },
        run: RunProps {
            size: Some(size),
            font: Some("+mn-lt".into()),
            fill: Some(Fill::solid(ColorRef::scheme(SchemeSlot::Tx1))),
            kern: Some(12.0),
            ..Default::default()
        },
    }
}

/// Master text styles: title, body (bulleted levels) and other.
pub fn master_styles() -> (ListStyle, ListStyle, ListStyle) {
    let mut title = ListStyle::default();
    title.set(
        0,
        LevelStyle {
            para: ParaProps {
                align: Some(crate::text::Align::Left),
                line_spacing: Some(Spacing::Pct(0.9)),
                space_before: Some(Spacing::Pts(0.0)),
                bullet: Some(Bullet::None),
                ..Default::default()
            },
            run: RunProps {
                size: Some(44.0),
                font: Some("+mj-lt".into()),
                fill: Some(Fill::solid(ColorRef::scheme(SchemeSlot::Tx1))),
                kern: Some(12.0),
                ..Default::default()
            },
        },
    );
    let mut body = ListStyle::default();
    let sizes = [28.0, 24.0, 20.0, 18.0, 18.0, 18.0, 18.0, 18.0, 18.0];
    for (i, sz) in sizes.iter().enumerate() {
        let margin = 18.0 + 36.0 * i as f64;
        body.set(i as u8, level(*sz, margin, -18.0, if i == 0 { 10.0 } else { 5.0 }));
    }
    let mut other = ListStyle::default();
    for i in 0..9u8 {
        other.set(
            i,
            LevelStyle {
                para: ParaProps { margin_left: Some(36.0 * i as f64), align: Some(crate::text::Align::Left), ..Default::default() },
                run: RunProps {
                    size: Some(18.0),
                    font: Some("+mn-lt".into()),
                    fill: Some(Fill::solid(ColorRef::scheme(SchemeSlot::Tx1))),
                    kern: Some(12.0),
                    ..Default::default()
                },
            },
        );
    }
    (title, body, other)
}

/// Presentation default text style for free shapes and text boxes.
pub fn default_text_style() -> ListStyle {
    let mut s = ListStyle::default();
    for i in 0..9u8 {
        s.set(
            i,
            LevelStyle {
                para: ParaProps { margin_left: Some(36.0 * i as f64), align: Some(crate::text::Align::Left), ..Default::default() },
                run: RunProps {
                    size: Some(18.0),
                    font: Some("+mn-lt".into()),
                    fill: Some(Fill::solid(ColorRef::scheme(SchemeSlot::Tx1))),
                    kern: Some(12.0),
                    ..Default::default()
                },
            },
        );
    }
    s
}

/// The master and its eleven standard layouts for a slide size, with ids starting at `first_id`.
/// Returns the master and the next free id.
pub fn build_master(size: Size, theme: Theme, first_id: u32) -> (Master, u32) {
    let (w, h) = (size.width, size.height);
    let sx = w / 960.0;
    let sy = h / 540.0;
    let r = |x: f64, y: f64, ww: f64, hh: f64| (x * sx, y * sy, ww * sx, hh * sy);
    let mut b = B { next: first_id };
    let footers = |b: &mut B| {
        let (x1, y1, w1, h1) = r(60.0, 500.0, 216.0, 28.0);
        let (x2, _, w2, _) = r(318.0, 500.0, 324.0, 28.0);
        let (x3, _, w3, _) = r(684.0, 500.0, 216.0, 28.0);
        let mut d = b.shape("Date Placeholder", PhType::Date, 10, [x1, y1, w1, h1]);
        let mut f = b.shape("Footer Placeholder", PhType::Footer, 11, [x2, y1, w2, h1]);
        let mut n = b.shape("Slide Number Placeholder", PhType::SlideNum, 12, [x3, y1, w3, h1]);
        let small = |align| {
            let mut ls = ListStyle::default();
            ls.set(
                0,
                LevelStyle {
                    para: ParaProps { align: Some(align), ..Default::default() },
                    run: RunProps {
                        size: Some(12.0),
                        fill: Some(Fill::solid(ColorRef::scheme(SchemeSlot::Tx1).with(deckcraft_color::ColorTransform::Tint(75000)))),
                        ..Default::default()
                    },
                },
            );
            ls
        };
        for (s, a) in [(&mut d, crate::text::Align::Left), (&mut f, crate::text::Align::Center), (&mut n, crate::text::Align::Right)] {
            if let Some(t) = s.text.as_mut() {
                t.body.anchor = Some(Anchor::Middle);
                t.list_style = small(a);
            }
        }
        if let Some(t) = n.text.as_mut() {
            t.paragraphs = vec![crate::text::Paragraph {
                runs: vec![crate::text::Run {
                    text: "‹#›".into(),
                    props: RunProps::default(),
                    kind: crate::text::RunKind::Field { field: "slidenum".into() },
                }],
                ..Default::default()
            }];
        }
        vec![d, f, n]
    };
    let title_box = |b: &mut B| {
        let (x, y, ww, hh) = r(60.0, 29.0, 840.0, 104.0);
        let mut s = b.shape("Title 1", PhType::Title, 0, [x, y, ww, hh]);
        s.text = Some(body_with(Some(Anchor::Middle), Some(AutoFit::Shrink { font_scale: Some(1.0), line_reduction: 0.0 })));
        s
    };
    let body_box = |b: &mut B, name: &str, idx: u32, x: f64, y: f64, ww: f64, hh: f64| {
        let (x, y, ww, hh) = r(x, y, ww, hh);
        let mut s = b.shape(name, PhType::Body, idx, [x, y, ww, hh]);
        s.text = Some(body_with(Some(Anchor::Top), Some(AutoFit::Shrink { font_scale: Some(1.0), line_reduction: 0.0 })));
        s
    };

    // Master placeholders.
    let mut mshapes = vec![title_box(&mut b), body_box(&mut b, "Text Placeholder 2", 1, 60.0, 144.0, 840.0, 343.0)];
    mshapes.extend(footers(&mut b));
    for s in &mut mshapes {
        let kind = s.ph_type();
        if let Some(t) = s.text.as_mut()
            && t.paragraphs.is_empty()
        {
            let prompt = match kind {
                Some(PhType::Title) => "Click to edit Master title style",
                Some(PhType::Body) => "Click to edit Master text styles",
                _ => "",
            };
            if !prompt.is_empty() {
                t.paragraphs = vec![crate::text::Paragraph::new(prompt)];
            }
        }
    }

    let mut layouts = vec![];
    let mut lay = |b: &mut B, name: &str, kind: LayoutType, shapes: Vec<Shape>| {
        b.next += 1;
        layouts.push(Layout {
            id: LayoutId(b.next),
            name: name.into(),
            kind,
            shapes,
            background: None,
            show_master_shapes: true,
            preserve: true,
            color_map: None,
            raw_ext: None,
        });
    };

    // Title Slide
    {
        let (x, y, ww, hh) = r(120.0, 88.0, 720.0, 188.0);
        let mut t = b.shape("Title 1", PhType::CtrTitle, 0, [x, y, ww, hh]);
        let mut ls = ListStyle::default();
        ls.set(
            0,
            LevelStyle {
                para: ParaProps { align: Some(crate::text::Align::Center), ..Default::default() },
                run: RunProps { size: Some(60.0), ..Default::default() },
            },
        );
        t.text = Some(TextBody {
            body: BodyProps {
                anchor: Some(Anchor::Bottom),
                autofit: Some(AutoFit::Shrink { font_scale: Some(1.0), line_reduction: 0.0 }),
                ..Default::default()
            },
            list_style: ls,
            paragraphs: vec![],
        });
        let (x, y, ww, hh) = r(120.0, 284.0, 720.0, 130.0);
        let mut st = b.shape("Subtitle 2", PhType::SubTitle, 1, [x, y, ww, hh]);
        let mut ls = ListStyle::default();
        ls.set(
            0,
            LevelStyle {
                para: ParaProps {
                    align: Some(crate::text::Align::Center),
                    margin_left: Some(0.0),
                    indent: Some(0.0),
                    bullet: Some(Bullet::None),
                    ..Default::default()
                },
                run: RunProps { size: Some(24.0), ..Default::default() },
            },
        );
        st.text = Some(TextBody {
            body: BodyProps {
                anchor: Some(Anchor::Top),
                autofit: Some(AutoFit::Shrink { font_scale: Some(1.0), line_reduction: 0.0 }),
                ..Default::default()
            },
            list_style: ls,
            paragraphs: vec![],
        });
        let mut v = vec![t, st];
        v.extend(footers(&mut b));
        lay(&mut b, "Title Slide", LayoutType::Title, v);
    }
    // Title and Content
    {
        let t = title_box(&mut b);
        let c = body_with_ph(body_box(&mut b, "Content Placeholder 2", 1, 60.0, 144.0, 840.0, 343.0), PhType::Obj);
        let mut v = vec![t, c];
        v.extend(footers(&mut b));
        lay(&mut b, "Title and Content", LayoutType::TitleAndContent, v);
    }
    // Section Header
    {
        let (x, y, ww, hh) = r(65.0, 135.0, 830.0, 225.0);
        let mut t = b.shape("Title 1", PhType::Title, 0, [x, y, ww, hh]);
        let mut ls = ListStyle::default();
        ls.set(0, LevelStyle { run: RunProps { size: Some(60.0), ..Default::default() }, ..Default::default() });
        t.text = Some(TextBody { body: BodyProps { anchor: Some(Anchor::Bottom), ..Default::default() }, list_style: ls, paragraphs: vec![] });
        let mut s = body_box(&mut b, "Text Placeholder 2", 1, 65.0, 362.0, 830.0, 118.0);
        let mut ls = ListStyle::default();
        ls.set(
            0,
            LevelStyle {
                para: ParaProps { margin_left: Some(0.0), indent: Some(0.0), bullet: Some(Bullet::None), ..Default::default() },
                run: RunProps {
                    size: Some(24.0),
                    fill: Some(Fill::solid(ColorRef::scheme(SchemeSlot::Tx1).with(deckcraft_color::ColorTransform::Tint(82000)))),
                    ..Default::default()
                },
            },
        );
        if let Some(tb) = s.text.as_mut() {
            tb.list_style = ls;
        }
        let mut v = vec![t, s];
        v.extend(footers(&mut b));
        lay(&mut b, "Section Header", LayoutType::SectionHeader, v);
    }
    // Two Content
    {
        let t = title_box(&mut b);
        let l = body_with_ph(body_box(&mut b, "Content Placeholder 2", 1, 60.0, 144.0, 412.0, 343.0), PhType::Obj);
        let rr = body_with_ph(body_box(&mut b, "Content Placeholder 3", 2, 488.0, 144.0, 412.0, 343.0), PhType::Obj);
        let mut v = vec![t, l, rr];
        v.extend(footers(&mut b));
        lay(&mut b, "Two Content", LayoutType::TwoContent, v);
    }
    // Comparison
    {
        let t = title_box(&mut b);
        let head = |b: &mut B, name: &str, idx: u32, x: f64| {
            let (x, y, ww, hh) = r(x, 133.0, 412.0, 62.0);
            let mut s = b.shape(name, PhType::Body, idx, [x, y, ww, hh]);
            let mut ls = ListStyle::default();
            ls.set(
                0,
                LevelStyle {
                    para: ParaProps { margin_left: Some(0.0), indent: Some(0.0), bullet: Some(Bullet::None), ..Default::default() },
                    run: RunProps { size: Some(24.0), bold: Some(true), ..Default::default() },
                },
            );
            s.text = Some(TextBody { body: BodyProps { anchor: Some(Anchor::Bottom), ..Default::default() }, list_style: ls, paragraphs: vec![] });
            s
        };
        let h1 = head(&mut b, "Text Placeholder 2", 1, 60.0);
        let c1 = body_with_ph(body_box(&mut b, "Content Placeholder 3", 2, 60.0, 198.0, 412.0, 289.0), PhType::Obj);
        let h2 = head(&mut b, "Text Placeholder 4", 3, 488.0);
        let c2 = body_with_ph(body_box(&mut b, "Content Placeholder 5", 4, 488.0, 198.0, 412.0, 289.0), PhType::Obj);
        let mut v = vec![t, h1, c1, h2, c2];
        v.extend(footers(&mut b));
        lay(&mut b, "Comparison", LayoutType::Comparison, v);
    }
    // Title Only
    {
        let mut v = vec![title_box(&mut b)];
        v.extend(footers(&mut b));
        lay(&mut b, "Title Only", LayoutType::TitleOnly, v);
    }
    // Blank
    {
        let v = footers(&mut b);
        lay(&mut b, "Blank", LayoutType::Blank, v);
    }
    // Content with Caption
    {
        let (x, y, ww, hh) = r(60.0, 36.0, 322.0, 126.0);
        let mut t = b.shape("Title 1", PhType::Title, 0, [x, y, ww, hh]);
        let mut ls = ListStyle::default();
        ls.set(0, LevelStyle { run: RunProps { size: Some(32.0), ..Default::default() }, ..Default::default() });
        t.text = Some(TextBody { body: BodyProps { anchor: Some(Anchor::Bottom), ..Default::default() }, list_style: ls, paragraphs: vec![] });
        let c = body_with_ph(body_box(&mut b, "Content Placeholder 2", 1, 412.0, 78.0, 488.0, 384.0), PhType::Obj);
        let mut cap = body_box(&mut b, "Text Placeholder 3", 2, 60.0, 162.0, 322.0, 300.0);
        let mut ls = ListStyle::default();
        ls.set(
            0,
            LevelStyle {
                para: ParaProps { margin_left: Some(0.0), indent: Some(0.0), bullet: Some(Bullet::None), ..Default::default() },
                run: RunProps { size: Some(16.0), ..Default::default() },
            },
        );
        if let Some(tb) = cap.text.as_mut() {
            tb.list_style = ls;
        }
        let mut v = vec![t, c, cap];
        v.extend(footers(&mut b));
        lay(&mut b, "Content with Caption", LayoutType::ContentWithCaption, v);
    }
    // Picture with Caption
    {
        let (x, y, ww, hh) = r(60.0, 36.0, 322.0, 126.0);
        let mut t = b.shape("Title 1", PhType::Title, 0, [x, y, ww, hh]);
        let mut ls = ListStyle::default();
        ls.set(0, LevelStyle { run: RunProps { size: Some(32.0), ..Default::default() }, ..Default::default() });
        t.text = Some(TextBody { body: BodyProps { anchor: Some(Anchor::Bottom), ..Default::default() }, list_style: ls, paragraphs: vec![] });
        let (x, y, ww, hh) = r(412.0, 78.0, 488.0, 384.0);
        let p = b.shape("Picture Placeholder 2", PhType::Picture, 1, [x, y, ww, hh]);
        let mut cap = body_box(&mut b, "Text Placeholder 3", 2, 60.0, 162.0, 322.0, 300.0);
        let mut ls = ListStyle::default();
        ls.set(
            0,
            LevelStyle {
                para: ParaProps { margin_left: Some(0.0), indent: Some(0.0), bullet: Some(Bullet::None), ..Default::default() },
                run: RunProps { size: Some(16.0), ..Default::default() },
            },
        );
        if let Some(tb) = cap.text.as_mut() {
            tb.list_style = ls;
        }
        let mut v = vec![t, p, cap];
        v.extend(footers(&mut b));
        lay(&mut b, "Picture with Caption", LayoutType::PictureWithCaption, v);
    }
    // Title and Vertical Text
    {
        let t = title_box(&mut b);
        let mut c = body_box(&mut b, "Vertical Text Placeholder 2", 1, 60.0, 144.0, 840.0, 343.0);
        if let Some(tb) = c.text.as_mut() {
            tb.body.vert = Some(TextDir::EaVertical);
        }
        if let Some(p) = c.ph.as_mut() {
            p.vertical = true;
        }
        let mut v = vec![t, c];
        v.extend(footers(&mut b));
        lay(&mut b, "Title and Vertical Text", LayoutType::TitleAndVerticalText, v);
    }
    // Vertical Title and Text
    {
        let (x, y, ww, hh) = r(687.0, 29.0, 213.0, 458.0);
        let mut t = b.shape("Vertical Title 1", PhType::Title, 0, [x, y, ww, hh]);
        t.text = Some(TextBody { body: BodyProps { vert: Some(TextDir::EaVertical), ..Default::default() }, ..Default::default() });
        if let Some(p) = t.ph.as_mut() {
            p.vertical = true;
        }
        let mut c = body_box(&mut b, "Vertical Text Placeholder 2", 1, 60.0, 29.0, 612.0, 458.0);
        if let Some(tb) = c.text.as_mut() {
            tb.body.vert = Some(TextDir::EaVertical);
        }
        if let Some(p) = c.ph.as_mut() {
            p.vertical = true;
        }
        let mut v = vec![t, c];
        v.extend(footers(&mut b));
        lay(&mut b, "Vertical Title and Text", LayoutType::VerticalTitleAndText, v);
    }

    let (title_style, body_style, other_style) = master_styles();
    b.next += 1;
    let master = Master {
        id: MasterId(b.next),
        name: theme.name.clone(),
        theme,
        shapes: mshapes,
        background: Some(Background::Ref { idx: 1001, color: ColorRef::scheme(SchemeSlot::Bg1) }),
        title_style,
        body_style,
        other_style,
        layouts,
        color_map: vec![],
        preserve: false,
        raw_ext: None,
    };
    (master, b.next + 1)
}

fn body_with_ph(mut s: Shape, kind: PhType) -> Shape {
    if let Some(p) = s.ph.as_mut() {
        p.kind = kind;
    }
    s
}

/// A new presentation with one title slide.
pub fn new_presentation(theme: Option<Theme>) -> Presentation {
    blank_presentation(WIDE, theme.unwrap_or_default(), true)
}

pub fn blank_presentation(size: Size, theme: Theme, title_slide: bool) -> Presentation {
    let (master, next) = build_master(size, theme, 1);
    let first_layout = master.layouts.first().map(|l| l.id).unwrap_or_default();
    let mut p = Presentation {
        slide_size: size,
        notes_size: Size::new(540.0, 720.0),
        slides: vec![],
        masters: vec![Arc::new(master)],
        notes_master: None,
        handout_master: None,
        sections: vec![],
        custom_shows: vec![],
        show: Default::default(),
        header_footer: HeaderFooter::default(),
        props: Default::default(),
        default_text_style: default_text_style(),
        media: vec![],
        first_slide_number: 1,
        next_id: next.max(256),
        embedded_fonts: vec![],
        table_styles: vec![],
        tags: vec![],
        kept_parts: vec![],
        guides: vec![],
        raw_ext: None,
    };
    if title_slide {
        // A fresh deck starts at id 256 and this builds a handful of ids, so the slide always
        // fits; should that ever stop holding, the deck arrives without its title slide rather
        // than with a duplicated id.
        if let Some(s) = new_slide(&mut p, first_layout) {
            p.slides.push(Arc::new(s));
        }
    }
    p
}

/// A slide for `layout` with its placeholders instantiated (empty, inheriting position). `None`
/// when the deck has used every object id and cannot grow (#407).
pub fn new_slide(p: &mut Presentation, layout: LayoutId) -> Option<Slide> {
    let id = SlideId(p.alloc_id()?);
    let mut shapes = vec![];
    let lay_shapes: Vec<Shape> = p.layout(layout).map(|(_, l)| l.shapes.clone()).unwrap_or_default();
    for ls in lay_shapes {
        let Some(ph) = ls.ph.clone() else { continue };
        if ph.kind.is_footer_kind() {
            continue;
        }
        let sid = ShapeId(p.alloc_id()?);
        shapes.push(Shape {
            id: sid,
            name: ls.name.clone(),
            xfrm: None,
            kind: ShapeKind::Shape,
            ph: Some(ph),
            text: Some(TextBody::default()),
            ..Default::default()
        });
    }
    Some(Slide { id, layout, shapes, ..Default::default() })
}

/// Layout for a kind in the first master.
pub fn layout_of_kind(p: &Presentation, kind: LayoutType) -> Option<LayoutId> {
    p.masters.first().and_then(|m| m.layouts.iter().find(|l| l.kind == kind)).map(|l| l.id)
}
