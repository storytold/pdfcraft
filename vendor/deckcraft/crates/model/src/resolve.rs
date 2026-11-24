//! Inheritance: effective position, fill, line, body and text properties of a shape, following
//! slide → layout placeholder → master placeholder → master text styles → theme.

use deckcraft_color::{ColorScheme, Rgba, SchemeSlot};
use deckcraft_geom::Xfrm;

use crate::style::{ColorRef, Effects, Fill, Line, ShapeStyle};
use crate::text::{Anchor, AutoFit, BodyProps, LevelStyle, ListStyle, ParaProps, RunProps};
use crate::{Background, Layout, Master, PhType, Presentation, Shape, Slide};

/// Where a shape lives, for resolving inherited values.
#[derive(Clone, Copy)]
pub enum Owner {
    Slide,
    Layout,
    Master,
}

pub struct Ctx<'a> {
    pub pres: &'a Presentation,
    pub master: &'a Master,
    pub layout: Option<&'a Layout>,
    pub owner: Owner,
    pub scheme: ColorScheme,
    /// The colour map in force: the slide's override, else its layout's, else the master's.
    pub color_map: Vec<(String, String)>,
}

impl<'a> Ctx<'a> {
    pub fn for_slide(pres: &'a Presentation, slide: &Slide) -> Option<Ctx<'a>> {
        let (master, layout) = pres.master_for(slide)?;
        let map = slide.color_map.as_ref().or(layout.and_then(|l| l.color_map.as_ref())).unwrap_or(&master.color_map).clone();
        Some(Ctx { pres, master, layout, owner: Owner::Slide, scheme: master.scheme_with(&map), color_map: map })
    }
    pub fn for_layout(pres: &'a Presentation, master: &'a Master, layout: &'a Layout) -> Ctx<'a> {
        let map = layout.color_map.as_ref().unwrap_or(&master.color_map).clone();
        Ctx { pres, master, layout: Some(layout), owner: Owner::Layout, scheme: master.scheme_with(&map), color_map: map }
    }
    pub fn for_master(pres: &'a Presentation, master: &'a Master) -> Ctx<'a> {
        Ctx { pres, master, layout: None, owner: Owner::Master, scheme: master.scheme(), color_map: master.color_map.clone() }
    }
    pub fn color(&self, c: &ColorRef, ph: Option<Rgba>) -> Rgba {
        let c = match &c.base {
            crate::ColorBase::Scheme { slot } if matches!(slot, SchemeSlot::Bg1 | SchemeSlot::Tx1 | SchemeSlot::Bg2 | SchemeSlot::Tx2) => {
                ColorRef { base: crate::ColorBase::Scheme { slot: crate::map_slot(&self.color_map, *slot) }, mods: c.mods.clone() }
            }
            _ => c.clone(),
        };
        c.resolve(&self.master.theme.colors, ph)
    }
}

fn compatible(slide: PhType, other: PhType) -> bool {
    use PhType::*;
    match slide {
        Title | CtrTitle => matches!(other, Title | CtrTitle),
        SubTitle | Body | Obj | Chart | Table | ClipArt | Diagram | Media | Picture => {
            !matches!(other, Title | CtrTitle | Date | Footer | SlideNum | Header)
        }
        k => k == other,
    }
}

fn find_ph<'s>(shapes: &'s [Shape], shape: &Shape, by_idx: bool) -> Option<&'s Shape> {
    let ph = shape.ph.as_ref()?;
    if by_idx
        && ph.idx != 0
        && let Some(s) = shapes.iter().find(|s| s.ph.as_ref().is_some_and(|p| p.idx == ph.idx && compatible(ph.kind, p.kind)))
    {
        return Some(s);
    }
    // Same type first, then a compatible one.
    shapes
        .iter()
        .find(|s| s.ph.as_ref().is_some_and(|p| p.kind == ph.kind && (!by_idx || ph.idx == 0 || p.idx == ph.idx || ph.kind.is_title())))
        .or_else(|| shapes.iter().find(|s| s.ph.as_ref().is_some_and(|p| p.kind == ph.kind)))
        .or_else(|| shapes.iter().find(|s| s.ph.as_ref().is_some_and(|p| compatible(ph.kind, p.kind))))
}

fn master_ph(master: &Master, kind: PhType) -> Option<&Shape> {
    let want = match kind {
        PhType::Title | PhType::CtrTitle => PhType::Title,
        PhType::Date | PhType::Footer | PhType::SlideNum | PhType::Header => kind,
        _ => PhType::Body,
    };
    master.shapes.iter().find(|s| s.ph_type() == Some(want))
}

/// The layout and master placeholders a shape inherits from.
pub fn parents<'a>(ctx: &Ctx<'a>, shape: &Shape) -> (Option<&'a Shape>, Option<&'a Shape>) {
    let Some(ph) = shape.ph.as_ref() else { return (None, None) };
    match ctx.owner {
        Owner::Slide => {
            let lp = ctx.layout.and_then(|l| find_ph(&l.shapes, shape, true));
            let kind = lp.and_then(Shape::ph_type).unwrap_or(ph.kind);
            (lp, master_ph(ctx.master, kind))
        }
        Owner::Layout => (None, master_ph(ctx.master, ph.kind)),
        Owner::Master => (None, None),
    }
}

/// The text style group (title / body / other) a shape draws from. Text in a table takes the
/// master's other-text style, also when the table sits in a content placeholder, as in PowerPoint
/// for Mac (a table cell is 14 pt under an `otherStyle` of 14 pt, a `defaultTextStyle` of 26 pt and
/// a bulleted 36 pt `bodyStyle`, with or without `<p:ph idx="1"/>`, while a shape's or a text box's
/// text is 26 pt); other shapes take the presentation's default text style.
pub fn master_list_style<'a>(ctx: &Ctx<'a>, shape: &Shape) -> &'a ListStyle {
    if matches!(shape.kind, crate::ShapeKind::Table(_)) {
        return &ctx.master.other_style;
    }
    match shape.ph_type() {
        Some(PhType::Title | PhType::CtrTitle) => &ctx.master.title_style,
        Some(
            PhType::Body
            | PhType::Obj
            | PhType::SubTitle
            | PhType::Table
            | PhType::Chart
            | PhType::Diagram
            | PhType::Media
            | PhType::Picture
            | PhType::ClipArt,
        ) => &ctx.master.body_style,
        Some(_) => &ctx.master.other_style,
        None => &ctx.pres.default_text_style,
    }
}

pub fn xfrm(ctx: &Ctx, shape: &Shape) -> Xfrm {
    if let Some(x) = shape.xfrm {
        return x;
    }
    let (lp, mp) = parents(ctx, shape);
    lp.and_then(|s| s.xfrm).or_else(|| mp.and_then(|s| s.xfrm)).unwrap_or_else(|| {
        let sz = ctx.pres.slide_size;
        Xfrm::new(sz.width * 0.1, sz.height * 0.1, sz.width * 0.8, sz.height * 0.2)
    })
}

/// Effective fill and the style colour (`phClr`) to resolve it with.
pub fn fill(ctx: &Ctx, shape: &Shape) -> (Option<Fill>, Option<Rgba>) {
    let (lp, mp) = parents(ctx, shape);
    let style_fill = |s: &Shape| {
        s.style.as_ref().map(|st| {
            let (idx, col) = &st.fill_ref;
            let ph = ctx.color(col, None);
            let f = match *idx {
                0 => Some(Fill::None),
                i if i >= 1001 => ctx.master.theme.format.bg_fills.get((i - 1001) as usize).cloned(),
                i => ctx.master.theme.format.fills.get((i - 1) as usize).cloned(),
            };
            (f, Some(ph))
        })
    };
    for s in [Some(shape), lp, mp].into_iter().flatten() {
        if let Some(f) = &s.fill {
            // A gradient without stops (`a:gradFill` with no `a:gsLst`) takes them from the
            // gradient its style refers to, keeping its own direction, as PowerPoint draws it.
            if let Fill::Gradient(g) = f
                && g.stops.is_empty()
                && let Some((Some(Fill::Gradient(sg)), ph)) = style_fill(s)
            {
                let mut g = g.clone();
                g.stops = sg.stops;
                return (Some(Fill::Gradient(g)), ph);
            }
            return (Some(f.clone()), None);
        }
        if let Some(r) = style_fill(s) {
            return r;
        }
    }
    if shape.text_box {
        return (Some(Fill::None), None);
    }
    (None, None)
}

pub fn line(ctx: &Ctx, shape: &Shape) -> (Line, Option<Rgba>) {
    let (lp, mp) = parents(ctx, shape);
    let mut out = Line::default();
    let mut ph = None;
    for s in [Some(shape), lp, mp].into_iter().flatten() {
        if let Some(l) = &s.line {
            out.inherit(l);
        }
        if let Some(st) = &s.style {
            let (idx, col) = &st.line_ref;
            if ph.is_none() {
                ph = Some(ctx.color(col, None));
            }
            if *idx == 0 {
                out.inherit(&Line::none());
            } else if let Some(l) = ctx.master.theme.format.lines.get((*idx - 1) as usize) {
                out.inherit(l);
            }
            break;
        }
    }
    (out, ph)
}

pub fn effects(ctx: &Ctx, shape: &Shape) -> (Option<Effects>, Option<Rgba>) {
    let (lp, mp) = parents(ctx, shape);
    for s in [Some(shape), lp, mp].into_iter().flatten() {
        if let Some(e) = &s.effects {
            return (Some(e.clone()), None);
        }
        if let Some(st) = &s.style {
            let (idx, col) = &st.effect_ref;
            if *idx == 0 {
                return (None, None);
            }
            return (ctx.master.theme.format.effects.get((*idx - 1) as usize).cloned(), Some(ctx.color(col, None)));
        }
    }
    (None, None)
}

/// Turn a placeholder into a regular shape that looks the same: what it inherited from its layout
/// and master placeholders (position, fill, line, effects, style, body properties and the text
/// style of every outline level, the master's title or body style included) becomes its own, and
/// `ph` is cleared. A shape that is not a placeholder is left as it is.
///
/// Theme references (`+mj-lt`, scheme colours) stay references, so the copy still follows the
/// destination theme, as PowerPoint's default paste does.
pub fn detach_placeholder(ctx: &Ctx, shape: &mut Shape) {
    if shape.ph.is_none() {
        return;
    }
    // The text first: `level_style` reads the shape's own style, which changes below.
    let text = shape.text.as_ref().map(|_| (body(ctx, shape), (0..9).map(|l| Some(level_style(ctx, shape, l))).collect::<Vec<_>>()));
    let own = shape.clone();
    let (lp, mp) = parents(ctx, &own);
    let chain = [Some(&own), lp, mp].into_iter().flatten();

    // Each property comes from where `fill`, `line` and `effects` take it: the first shape in the
    // chain with an explicit value or a style. Lines merge explicit values up to the first styled
    // shape, whose style line fills the rest.
    let fill_src = chain.clone().find(|s| s.fill.is_some() || s.style.is_some());
    let effects_src = chain.clone().find(|s| s.effects.is_some() || s.style.is_some());
    let mut line = None;
    let mut line_src = None;
    for s in chain.clone() {
        if let Some(l) = &s.line {
            line.get_or_insert_with(Line::default).inherit(l);
        }
        if s.style.is_some() {
            line_src = Some(s);
            break;
        }
    }
    // Text takes its font reference from the shape's style or its layout placeholder's (a table's
    // from its own only), never the master's.
    let table = matches!(own.kind, crate::ShapeKind::Table(_));
    let font_src = own.style.as_ref().or(lp.filter(|_| !table).and_then(|s| s.style.as_ref()));

    // One style for the copy, each reference taken from the shape that supplied that property. A
    // property with an explicit value ignores its reference, except a gradient without stops,
    // which takes them from its own shape's style; a neutral reference (index 0) adds nothing.
    let style_of = |s: Option<&Shape>| s.and_then(|s| s.style.clone());
    let neutral = (0, ColorRef::scheme(SchemeSlot::Tx1));
    let style = chain.clone().any(|s| s.style.is_some()).then(|| ShapeStyle {
        fill_ref: style_of(fill_src).map_or_else(|| neutral.clone(), |st| st.fill_ref),
        line_ref: style_of(line_src).map_or_else(|| neutral.clone(), |st| st.line_ref),
        effect_ref: style_of(effects_src).map_or_else(|| neutral.clone(), |st| st.effect_ref),
        font_ref: font_src.map_or_else(|| ("none".into(), None), |st| st.font_ref.clone()),
    });

    shape.xfrm = Some(xfrm(ctx, &own));
    shape.fill = fill_src.and_then(|s| s.fill.clone());
    shape.effects = effects_src.and_then(|s| s.effects.clone());
    shape.line = line;
    shape.style = style;
    if let (Some(t), Some((body, levels))) = (shape.text.as_mut(), text) {
        t.body = body;
        t.list_style = ListStyle { levels };
    }
    shape.ph = None;
}

/// Body properties with defaults filled in.
pub fn body(ctx: &Ctx, shape: &Shape) -> BodyProps {
    let (lp, mp) = parents(ctx, shape);
    let mut b = shape.text.as_ref().map(|t| t.body.clone()).unwrap_or_default();
    for s in [lp, mp].into_iter().flatten() {
        if let Some(t) = &s.text {
            b.inherit(&t.body);
        }
    }
    b.inherit(&BodyProps {
        inset_l: Some(7.2),
        inset_t: Some(3.6),
        inset_r: Some(7.2),
        inset_b: Some(3.6),
        anchor: Some(if shape.ph.is_none() && !shape.text_box && !matches!(shape.kind, crate::ShapeKind::Table(_)) {
            Anchor::Middle
        } else {
            Anchor::Top
        }),
        anchor_ctr: Some(false),
        wrap: Some(true),
        autofit: Some(if shape.text_box { AutoFit::Shape } else { AutoFit::None }),
        vert: Some(crate::text::TextDir::Horizontal),
        rot: Some(0.0),
        columns: Some(1),
        col_spacing: Some(0.0),
        warp: None,
        warp_adj: Vec::new(),
        rtl_col: Some(false),
        upright: Some(false),
        space_first_last: Some(false),
        compat_ln_spc: None,
        vert_overflow: None,
        horz_overflow: None,
        raw3d: None,
    });
    b
}

/// The merged level style (paragraph + run defaults) for paragraphs at `level` in `shape`.
pub fn level_style(ctx: &Ctx, shape: &Shape, level: u8) -> LevelStyle {
    // A table's text inherits nothing from a placeholder the table sits in: PowerPoint draws it in
    // the other-text style even under list styles on the layout's and master's placeholders.
    let (lp, mp) = if matches!(shape.kind, crate::ShapeKind::Table(_)) { (None, None) } else { parents(ctx, shape) };
    let mut out = LevelStyle::default();
    let add = |out: &mut LevelStyle, ls: Option<&LevelStyle>| {
        if let Some(l) = ls {
            out.para.inherit(&l.para);
            out.run.inherit(&l.run);
        }
    };
    if let Some(t) = &shape.text {
        add(&mut out, t.list_style.level(level));
    }
    for s in [lp, mp].into_iter().flatten() {
        if let Some(t) = &s.text {
            add(&mut out, t.list_style.level(level));
        }
    }
    // Shape style font reference (theme font + colour) sits above the defaults.
    if let Some(st) = shape.style.as_ref().or(lp.and_then(|s| s.style.as_ref())) {
        let font = match st.font_ref.0.as_str() {
            "major" => Some("+mj-lt".to_string()),
            "minor" => Some("+mn-lt".to_string()),
            _ => None,
        };
        let r = RunProps { font, fill: st.font_ref.1.clone().map(Fill::solid), ..Default::default() };
        out.run.inherit(&r);
    }
    add(&mut out, master_list_style(ctx, shape).level(level));
    add(&mut out, ctx.pres.default_text_style.level(level));
    // Last resort.
    out.run.inherit(&RunProps { size: Some(18.0), font: Some("+mn-lt".into()), bold: Some(false), italic: Some(false), ..Default::default() });
    out.para.inherit(&ParaProps { margin_left: Some(0.0), indent: Some(0.0), align: Some(crate::text::Align::Left), ..Default::default() });
    out
}

/// Fully resolved run properties for `run` in paragraph `para` (explicit → paragraph style).
pub fn run(ctx: &Ctx, shape: &Shape, para: &crate::text::Paragraph, run: &RunProps) -> RunProps {
    let mut r = run.clone();
    // Paragraph `defRPr` fills gaps in the run, and the list style (including the layout
    // placeholder this shape inherits from) fills whatever is still unset.
    r.inherit(&para.def_props);
    let lvl = level_style(ctx, shape, para.level);
    r.inherit(&lvl.run);
    // Hyperlinked text is drawn in the theme's hyperlink colour and underlined, as in PowerPoint,
    // unless the run itself sets a colour (honoured since PowerPoint 2019) or an underline
    // (`u="none"` removes it). Colours and underlines from list styles don't count.
    if r.link.is_some() {
        // A colour or underline on the run, or on the same `defRPr` that carries the link, is
        // kept. A colour that arrived only from a list style is not.
        let from_def = run.link.is_none();
        if run.fill.is_none() && !(from_def && para.def_props.fill.is_some()) {
            r.fill = Some(Fill::solid(ColorRef::scheme(SchemeSlot::Hlink)));
        }
        if run.underline.is_none() && !(from_def && para.def_props.underline.is_some()) {
            r.underline = Some("sng".into());
        }
    }
    r
}

pub fn para(ctx: &Ctx, shape: &Shape, para: &crate::text::Paragraph) -> ParaProps {
    let mut p = para.props.clone();
    let lvl = level_style(ctx, shape, para.level);
    p.inherit(&lvl.para);
    p
}

/// Concrete font family for a run (theme references resolved).
pub fn font_family(ctx: &Ctx, r: &RunProps) -> String {
    let f = r.font.as_deref().unwrap_or("+mn-lt");
    let name = ctx.master.theme.font(f);
    if name.is_empty() { ctx.master.theme.fonts.minor.latin.clone() } else { name }
}

/// Text colour of resolved run props.
pub fn text_color(ctx: &Ctx, r: &RunProps) -> Rgba {
    match &r.fill {
        Some(Fill::Solid { color }) => ctx.color(color, None),
        Some(Fill::Gradient(g)) => g.stops.first().map(|s| ctx.color(&s.color, None)).unwrap_or(Rgba::BLACK),
        Some(Fill::None) => Rgba::TRANSPARENT,
        _ => ctx.color(&ColorRef::scheme(SchemeSlot::Tx1), None),
    }
}

/// The background fill (and its style colour) of a slide.
pub fn background(ctx: &Ctx, slide: Option<&Slide>) -> (Fill, Option<Rgba>) {
    let bg = slide.and_then(|s| s.background.as_ref()).or(ctx.layout.and_then(|l| l.background.as_ref())).or(ctx.master.background.as_ref());
    match bg {
        Some(Background::Fill { fill }) => (fill.clone(), None),
        Some(Background::Ref { idx, color }) => {
            let ph = ctx.color(color, None);
            let f = if *idx >= 1001 {
                ctx.master.theme.format.bg_fills.get((*idx - 1001) as usize).cloned()
            } else {
                ctx.master.theme.format.fills.get(idx.saturating_sub(1) as usize).cloned()
            };
            (f.unwrap_or(Fill::solid(ColorRef::rgb(ph))), Some(ph))
        }
        None => (Fill::solid(ColorRef::scheme(SchemeSlot::Bg1)), None),
    }
}

/// Should the layout/master graphics (non-placeholder shapes) show behind this slide?
pub fn show_master_shapes(slide: &Slide, layout: Option<&Layout>) -> (bool, bool) {
    let show_layout = slide.show_master_shapes;
    let show_master = show_layout && layout.is_none_or(|l| l.show_master_shapes);
    (show_layout, show_master)
}
